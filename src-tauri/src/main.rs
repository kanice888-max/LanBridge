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

use anyhow::{Context, Result};
use platform::traits::Platform;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    install_crash_hook();
    if let Err(error) = run_app() {
        write_startup_crash(&format!("{error:?}"));
        eprintln!("LanBridge failed to start: {error:?}");
        std::process::exit(1);
    }
}

fn run_app() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Load identity BEFORE Tauri so discovery can use the real device_id
    let platform: Box<dyn Platform> = Box::new(
        platform::windows::app_dirs::WinPlatform::new().context("failed to init platform")?,
    );
    let identity = pairing::DeviceIdentity::load_or_create(
        &platform
            .identity_key_path()
            .context("failed to get key path")?,
    )
    .context("failed to load identity")?;

    // Start discovery service in its own thread + tokio runtime
    let hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Device".to_string());
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
        .context("failed to initialize app state")?;

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
            commands::refresh_pending_returns,
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
            commands::set_transfer_speed_limit,
            commands::get_transfer_speed_limit,
            commands::open_in_file_manager,
            commands::get_local_network_info,
            commands::delete_sync_task,
            commands::get_transfer_progress,
            commands::get_sync_progress,
            commands::list_deferred_transfers,
            commands::resume_transfer,
            commands::get_task_peer_status,
            commands::cancel_transfer,
        ])
        .run(tauri::generate_context!())
        .context("error while running tauri application")?;
    Ok(())
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        write_startup_crash(&format!("panic: {panic_info}"));
    }));
}

fn write_startup_crash(message: &str) {
    let path = startup_crash_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{}] {}", now_ms(), message);
    }
}

fn startup_crash_log_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|home| home.join("AppData").join("Roaming"))
        })
        .unwrap_or_else(std::env::temp_dir)
        .join("LanBridge")
        .join("startup-crash.log")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
