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

use platform::traits::Platform;

fn main() {
    // Load identity BEFORE Tauri so discovery can use the real device_id
    let platform: Box<dyn Platform> =
        Box::new(platform::macos::app_dirs::MacPlatform::new().expect("failed to init platform"));
    let identity = pairing::DeviceIdentity::load_or_create(
        &platform
            .identity_key_path()
            .expect("failed to get key path"),
    )
    .expect("failed to load identity");

    // Start discovery service in its own thread + tokio runtime
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "Device".to_string());
    let pub_id = identity.public();
    let server = match transport::server::SyncServer::start_in_background(9527) {
        Ok(server) => Some(server),
        Err(e) => {
            eprintln!("failed to start sync server: {}", e);
            None
        }
    };
    if let Some(server) = &server {
        server.set_local_identity(pub_id.clone());
    }
    let advertised_port = server.as_ref().map_or(0, |server| server.port());
    let discovery = match transport::discovery::start_in_background(
        pub_id.device_id.clone(),
        hostname,
        pub_id.public_key.clone(),
        advertised_port,
    ) {
        Ok(discovery) => discovery,
        Err(e) => transport::DiscoveryState::failed(e.to_string()),
    };

    let app_state = app_state::AppState::new(identity, platform, discovery, server)
        .expect("failed to initialize app state");

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::get_identity,
            commands::start_pairing,
            commands::confirm_pairing_code,
            commands::approve_pairing,
            commands::get_paired_devices,
            commands::connect_peer,
            commands::connect_discovered_peer,
            commands::list_online_devices,
            commands::get_discovery_status,
            commands::check_network_environment,
            commands::send_task_invite,
            commands::poll_task_invite,
            commands::list_task_invites,
            commands::accept_task_invite,
            commands::reject_task_invite,
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
            commands::resolve_conflict_keep_both,
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
