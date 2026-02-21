use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

use serde::{Deserialize, Serialize};
use sysinfo::{System, SystemExt, ProcessExt};
use tauri::{AppHandle, Manager};

/// A single executable entry from the detectable games list.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GameExecutable {
    pub os: String,
    pub name: String,
}

/// A detectable game entry sourced from Discord's API.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DetectableGame {
    pub id: String,
    pub name: String,
    pub executables: Option<Vec<GameExecutable>>,
}

/// Payload emitted to the frontend when activity changes.
#[derive(Clone, Debug, Serialize)]
pub struct GameActivity {
    pub name: String,
    pub executable_name: Option<String>,
    pub is_running: bool,
}

/// Shared state for the scanner's watch list and current detected game.
pub struct ScannerState {
    pub watch_list: Mutex<Vec<DetectableGame>>,
    pub current_game: Mutex<Option<String>>,
    pub is_enabled: Mutex<bool>,
    pub notify: Arc<Notify>,
}

#[tauri::command]
pub fn set_scanner_enabled(state: tauri::State<'_, Arc<ScannerState>>, enabled: bool) {
    let mut is_enabled = state.is_enabled.lock().unwrap();
    if *is_enabled != enabled {
        *is_enabled = enabled;
        println!("[game_scanner] Scanner state set to: {}", enabled);
        // Wake up the loop immediately if enabled, or to process disable
        state.notify.notify_one();
    }
}

/// Fetches the Detectable Games list from Discord and filters it for the current OS.
async fn fetch_detectable_games() -> Result<Vec<DetectableGame>, reqwest::Error> {
    let url = "https://discord.com/api/v9/applications/detectable";
    let client = reqwest::Client::new();
    let response = client.get(url).send().await?;
    let games: Vec<DetectableGame> = response.json().await?;

    // We want to detect games regardless of OS (e.g., Windows games via Crossover on Mac)
    let mut filtered_games: Vec<DetectableGame> = games;

    // Push the mock calculator game for testing
    filtered_games.push(DetectableGame {
        id: "mock_calc_123".to_string(),
        name: "Calculator".to_string(),
        executables: Some(vec![GameExecutable {
            os: "all".to_string(),
            name: "Calculator".to_string(),
        }])
    });

    Ok(filtered_games)
}

/// Starts the background game scanner loop.
pub fn start(app: AppHandle, state: Arc<ScannerState>) {
    tauri::async_runtime::spawn(async move {
        let mut sys = System::new_all();
        let mut fetch_retry_interval = Duration::from_secs(5);

        // Intial fetch of the games list
        loop {
            match fetch_detectable_games().await {
                Ok(games) => {
                    println!("[game_scanner] Successfully fetched {} detectable games", games.len());
                    *state.watch_list.lock().unwrap() = games;
                    break;
                }
                Err(e) => {
                    println!("[game_scanner] Failed to fetch games: {}. Retrying in {:?}...", e, fetch_retry_interval);
                    tokio::time::sleep(fetch_retry_interval).await;
                    // Cap retry interval at 60 seconds
                    if fetch_retry_interval < Duration::from_secs(60) {
                        fetch_retry_interval *= 2;
                    }
                }
            }
        }



        loop {
            // Check enabled state first
            let is_enabled = *state.is_enabled.lock().unwrap();

            if !is_enabled {
                // If disabled, wait indefinitely for a notification (enable command)
                println!("[game_scanner] Scanner disabled, waiting...");
                state.notify.notified().await;
                println!("[game_scanner] Scanner woke up!");
                continue;
            }

            // If enabled, proceed with scan
            // Use specific refresh kind to prevent MacOS Objective-C null pointer panic 
            // from trying to fetch restricted process environments.
            sys.refresh_processes_specifics(
                sysinfo::ProcessRefreshKind::new()
            );

            let watch_list = state.watch_list.lock().unwrap().clone();
            let previous_game = state.current_game.lock().unwrap().clone();

            let mut detected_name: Option<String> = None;
            let mut detected_exe: Option<String> = None;

            // Check each process against the watch list
            for (_, process) in sys.processes() {
                let process_name = process.name();
                
                for game in &watch_list {
                    if let Some(executables) = &game.executables {
                        for exe in executables {
                            // Exact match (case insensitive), stripping '.exe' to match cleanly across OS APIs
                            let clean_exe = exe.name.trim_end_matches(".exe");
                            let clean_proc = process_name.trim_end_matches(".exe");
                            
                            if clean_exe.eq_ignore_ascii_case(clean_proc) {
                                println!("[game_scanner] Matched process '{}' to executable '{}' for game '{}'", process_name, exe.name, game.name);
                                detected_name = Some(game.name.clone());
                                detected_exe = Some(exe.name.clone());
                                break;
                            }
                        }
                    }
                    if detected_name.is_some() {
                        break;
                    }
                }
                if detected_name.is_some() {
                    break;
                }
            }

            // Only emit on state changes
            match (&previous_game, &detected_name) {
                (None, Some(name)) => {
                    // Game just started
                    println!("[game_scanner] Detected: {}", name);
                    let _ = app.emit_all(
                        "game-activity",
                        GameActivity {
                            name: name.clone(),
                            executable_name: detected_exe.clone(),
                            is_running: true,
                        },
                    );
                }
                (Some(prev), None) => {
                    // Game just stopped
                    println!("[game_scanner] Stopped: {}", prev);
                    let _ = app.emit_all(
                        "game-activity",
                        GameActivity {
                            name: prev.clone(),
                            executable_name: None,
                            is_running: false,
                        },
                    );
                }
                (Some(prev), Some(name)) if prev != name => {
                    // Switched games
                    println!("[game_scanner] Switched: {} -> {}", prev, name);
                    let _ = app.emit_all(
                        "game-activity",
                        GameActivity {
                            name: name.clone(),
                            executable_name: detected_exe.clone(),
                            is_running: true,
                        },
                    );
                }
                _ => {
                    // No change — don't emit
                }
            }

            // Update current state
            *state.current_game.lock().unwrap() = detected_name;

            // Wait for 15s OR a notification (e.g. disable command or instant re-scan)
            if tokio::time::timeout(Duration::from_secs(15), state.notify.notified()).await.is_ok() {
                println!("[game_scanner] Scan interrupt received");
            }
        }
    });
}
