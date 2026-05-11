#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app_state;
mod commands;
mod core;
mod history;
mod pairing;
mod platform;
mod state;
mod transport;

fn main() {
    let app_state = app_state::AppState::new().expect("failed to initialize app state");

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_identity,
            commands::start_pairing,
            commands::confirm_pairing_code,
            commands::approve_pairing,
            commands::get_paired_devices,
            commands::connect_peer,
            commands::create_sync_task,
            commands::list_sync_tasks,
            commands::get_sync_task,
            commands::toggle_task_enabled,
            commands::scan_task,
            commands::sync_now,
            commands::list_pending_returns,
            commands::get_pending_count,
            commands::execute_return_sync,
            commands::detect_conflicts,
            commands::resolve_conflict_overwrite,
            commands::list_history,
            commands::restore_history_entry,
            commands::cleanup_history,
            commands::list_logs,
            commands::write_log,
            commands::get_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
