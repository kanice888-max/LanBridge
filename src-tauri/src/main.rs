#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod app_settings;
mod app_state;
mod commands;
mod core;
mod diagnostics;
mod history;
mod pairing;
mod platform;
mod state;
mod transport;
mod update;

use anyhow::{Context, Result};
use platform::traits::Platform;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{
    CustomMenuItem, Icon, Manager, PhysicalSize, Size, SystemTray, SystemTrayEvent, SystemTrayMenu,
    Window, WindowEvent,
};

const TRAY_OPEN_ID: &str = "open";
const TRAY_QUIT_ID: &str = "quit";
const DESIGN_ASPECT_RATIO: f64 = 863.0 / 561.0;
static WINDOW_RATIO_RESIZE_GUARD: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
struct TaskFilesChangedEvent {
    task_id: String,
    revision: u64,
    reason: String,
}

#[derive(Clone, Serialize)]
struct TaskTransferActivityEvent {
    task_id: String,
}

#[cfg(target_os = "macos")]
fn create_platform() -> Result<Box<dyn Platform>> {
    Ok(Box::new(
        platform::macos::app_dirs::MacPlatform::new().context("failed to init platform")?,
    ))
}

#[cfg(target_os = "windows")]
fn create_platform() -> Result<Box<dyn Platform>> {
    Ok(Box::new(
        platform::windows::app_dirs::WinPlatform::new().context("failed to init platform")?,
    ))
}

#[cfg(target_os = "macos")]
fn default_hostname() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "Device".to_string())
}

#[cfg(target_os = "windows")]
fn default_hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Device".to_string())
}

fn main() {
    install_crash_hook();
    diagnostics::record_operation("process_start", "LanBridge main entered");
    if let Err(error) = run_app() {
        diagnostics::record_startup_error(&format!("{error:?}"));
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
    diagnostics::record_operation("tracing_initialized", "tracing subscriber initialized");

    // Load identity BEFORE Tauri so discovery can use the real device_id
    let platform = create_platform()?;
    diagnostics::record_operation("platform_initialized", std::env::consts::OS);
    let identity = pairing::DeviceIdentity::load_or_create(
        &platform
            .identity_key_path()
            .context("failed to get key path")?,
    )
    .context("failed to load identity")?;
    diagnostics::record_operation(
        "identity_loaded",
        format!("device_id={}", identity.public().device_id),
    );

    // Start discovery service in its own thread + tokio runtime
    let hostname = default_hostname();
    let pub_id = identity.public();
    let settings = app_settings::load(platform.as_ref()).unwrap_or_else(|error| {
        diagnostics::record_operation("settings_load_failed", error.to_string());
        app_settings::AppSettings::default()
    });
    let server = match transport::server::SyncServer::start_in_background_with_fallback(9527) {
        Ok(server) => Some(server),
        Err(e) => {
            diagnostics::record_operation("sync_server_start_failed", e.to_string());
            eprintln!("failed to start sync server: {}", e);
            None
        }
    };
    if let Some(server) = &server {
        server.set_local_identity(pub_id.clone());
    }
    let advertised_port = server.as_ref().map_or(0, |server| server.port());
    diagnostics::record_operation("sync_server_ready", format!("port={advertised_port}"));
    let discovery = if settings.discovery_enabled {
        match transport::discovery::start_in_background(
            pub_id.device_id.clone(),
            hostname,
            pub_id.public_key.clone(),
            advertised_port,
        ) {
            Ok(discovery) => discovery,
            Err(e) => {
                diagnostics::record_operation("discovery_start_failed", e.to_string());
                transport::DiscoveryState::failed(e.to_string())
            }
        }
    } else {
        diagnostics::record_operation("discovery_disabled", "automatic discovery is disabled");
        transport::DiscoveryState::disabled()
    };

    let app_state = app_state::AppState::new(identity, platform, discovery, server)
        .context("failed to initialize app state")?;
    diagnostics::record_operation("app_state_initialized", "state ready before tauri builder");

    tauri::Builder::default()
        .system_tray(build_system_tray())
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
            commands::inspect_task_folder,
            commands::send_task_invite,
            commands::poll_task_invite,
            commands::list_task_invites,
            commands::accept_task_invite,
            commands::reject_task_invite,
            commands::create_sync_task,
            commands::list_sync_tasks,
            commands::get_sync_task,
            commands::toggle_task_enabled,
            commands::list_ready_auto_sync_tasks,
            commands::list_task_access_issues,
            commands::get_task_file_list_refresh_hint,
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
            commands::get_diagnostic_report,
            commands::write_log,
            commands::get_settings,
            commands::get_app_settings,
            commands::set_discovery_enabled,
            commands::check_for_updates,
            commands::open_project_github,
            commands::open_available_update_release,
            commands::hide_main_window_to_tray,
            commands::show_main_window,
            commands::quit_app,
            commands::set_transfer_speed_limit,
            commands::get_transfer_speed_limit,
            commands::open_in_file_manager,
            commands::get_local_network_info,
            commands::delete_task_entry,
            commands::import_task_entries,
            commands::delete_sync_task,
            commands::get_transfer_progress,
            commands::has_active_transfers,
            commands::get_sync_progress,
            commands::list_deferred_transfers,
            commands::resume_transfer,
            commands::get_task_peer_status,
            commands::open_local_network_settings,
            commands::disconnect_task_peer,
            commands::reconnect_task_peer,
            commands::get_window_cursor_position,
            commands::cancel_transfer,
        ])
        .on_window_event(|event| match event.event() {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = event.window().emit("lanbridge-close-requested", ());
            }
            WindowEvent::Resized(size) if event.window().label() == "main" => {
                enforce_design_aspect_ratio(event.window(), size.width, size.height);
            }
            _ => {}
        })
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::LeftClick { .. } => {
                let _ = show_main_window(app);
            }
            SystemTrayEvent::MenuItemClick { id, .. } if id.as_str() == TRAY_OPEN_ID => {
                let _ = show_main_window(app);
            }
            SystemTrayEvent::MenuItemClick { id, .. } if id.as_str() == TRAY_QUIT_ID => {
                app.exit(0);
            }
            _ => {}
        })
        .setup(|app| {
            diagnostics::record_operation("tauri_setup", "setup hook entered");
            let app_handle = app.handle();
            let state = app.state::<app_state::AppState>();
            let transfer_activity = state.transfer_activity.clone();
            let activity_app_handle = app_handle.clone();
            transfer_activity.set_callback(move |task_id| {
                let payload = TaskTransferActivityEvent { task_id };
                if let Err(error) = activity_app_handle.emit_all("lanbridge://task-transfer-activity", payload) {
                    tracing::warn!(error = %error, "failed to publish task transfer activity to UI");
                }
            });
            if let Some(server) = &state._server {
                let server_activity = state.transfer_activity.clone();
                server.set_transfer_activity_notifier(move |task_id| {
                    if let Ok(task_id) = uuid::Uuid::parse_str(&task_id) {
                        server_activity.publish(task_id);
                    }
                });
                let refresh_tracker = state.file_list_refresh.clone();
                server.set_receive_commit_notifier(move |task_id, kind| {
                    let Ok(parsed_task_id) = uuid::Uuid::parse_str(&task_id) else {
                        tracing::warn!(task_id = %task_id, "ignored receive refresh for invalid task id");
                        return;
                    };
                    let revision = refresh_tracker.mark(parsed_task_id, "receive_committed");
                    let payload = TaskFilesChangedEvent {
                        task_id,
                        revision,
                        reason: kind.event_reason().to_string(),
                    };
                    if let Err(error) = app_handle.emit_all("lanbridge://task-files-changed", payload) {
                        tracing::warn!(error = %error, "failed to publish received file change to UI");
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("error while running tauri application")?;
    diagnostics::record_operation("tauri_run_returned", "tauri run loop returned");
    Ok(())
}

fn enforce_design_aspect_ratio(window: &Window, width: u32, height: u32) {
    if width == 0 || height == 0 {
        return;
    }
    if WINDOW_RATIO_RESIZE_GUARD.swap(true, Ordering::SeqCst) {
        return;
    }

    let ratio = width as f64 / height as f64;
    let (next_width, next_height) = if ratio > DESIGN_ASPECT_RATIO {
        ((height as f64 * DESIGN_ASPECT_RATIO).round() as u32, height)
    } else {
        (width, (width as f64 / DESIGN_ASPECT_RATIO).round() as u32)
    };

    let needs_resize = width.abs_diff(next_width) > 2 || height.abs_diff(next_height) > 2;
    if needs_resize {
        let _ = window.set_size(Size::Physical(PhysicalSize {
            width: next_width,
            height: next_height,
        }));
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(80));
            WINDOW_RATIO_RESIZE_GUARD.store(false, Ordering::SeqCst);
        });
    } else {
        WINDOW_RATIO_RESIZE_GUARD.store(false, Ordering::SeqCst);
    }
}

fn build_system_tray() -> SystemTray {
    let menu = SystemTrayMenu::new()
        .add_item(CustomMenuItem::new(TRAY_OPEN_ID, "打开主窗口"))
        .add_item(CustomMenuItem::new(TRAY_QUIT_ID, "退出"));
    SystemTray::new()
        .with_menu(menu)
        .with_icon(Icon::Raw(tray_icon_bytes().to_vec()))
}

fn tray_icon_bytes() -> &'static [u8] {
    #[cfg(target_os = "macos")]
    {
        return include_bytes!("../icons/tray-template.png");
    }

    #[cfg(not(target_os = "macos"))]
    include_bytes!("../icons/32x32.png")
}

#[cfg(test)]
mod tray_tests {
    #[test]
    fn embedded_tray_icon_is_a_non_empty_png() {
        let icon = super::tray_icon_bytes();
        assert!(icon.len() > 8);
        assert_eq!(&icon[..8], b"\x89PNG\r\n\x1a\n");
    }
}

fn show_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_window("main") {
        window.show()?;
        window.unminimize()?;
        window.set_focus()?;
    }
    Ok(())
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        diagnostics::record_panic(&format!("{panic_info}"));
    }));
}
