#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

mod core;
mod history;
mod platform;
mod state;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to LAN Folder Sync.", name)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
