//! `MouseFlock.exe`: optional Tauri shell. Tray, autostart, pipe client.
//!
//! Nothing changes when it is closed. No engine logic here: it shows what
//! the service reports and passes the user's choices back as UI commands.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use std::sync::Arc;

use tauri::Manager;

mod checks;
mod commands;
mod link;
mod pipe;
mod place;
mod shell;
mod view;

fn main() {
    tauri::Builder::default()
        // First, so a second launch goes no further than showing the window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(shell) = app.try_state::<Arc<shell::Shell>>() {
                shell.show_main(None);
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("MouseFlock")
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::view,
            commands::route,
            commands::open_pairing,
            commands::close_pairing,
            commands::join,
            commands::remove_peer,
            commands::undo,
            commands::forget_layout,
        ])
        .on_window_event(shell::on_window_event)
        .setup(|app| shell::setup(app.handle()))
        .run(tauri::generate_context!())
        .expect("failed to run MouseFlock");
}
