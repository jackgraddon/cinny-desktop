#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "macos")]
mod menu;

mod game_scanner;

use tauri::{utils::config::AppUrl, WindowUrl};

fn main() {
    let port = 44548;

    let mut context = tauri::generate_context!();

    #[cfg(not(debug_assertions))]
    {
        let url = format!("http://localhost:{}", port).parse().unwrap();
        let window_url = WindowUrl::External(url);
        // rewrite the config so the IPC is enabled on this URL
        context.config_mut().build.dist_dir = AppUrl::Url(window_url.clone());
        context.config_mut().build.dev_path = AppUrl::Url(window_url.clone());
    }

    let mut builder = tauri::Builder::default();

    #[cfg(target_os = "macos")]
    {
        builder = builder.menu(menu::menu());
    }

    #[cfg(not(debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_localhost::Builder::new(port).build());
    }

    let mut scanner_state = game_scanner::ScannerState {
        watch_list: std::sync::Mutex::new(Vec::new()),
        current_game: std::sync::Mutex::new(None),
        is_enabled: std::sync::Mutex::new(false),
        notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    };
    let scanner_state_arc = std::sync::Arc::new(scanner_state);

    builder
        .manage(scanner_state_arc.clone())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(move |app| {
            game_scanner::start(app.handle().clone(), scanner_state_arc);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            game_scanner::set_scanner_enabled
        ])
        .run(context)
        .expect("error while building tauri application")
}
