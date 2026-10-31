/* EasySSH v4 (Tauri 2) — 应用入口 + PoC 命令 */
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ssh;

use parking_lot::Mutex;
use tauri::{Emitter, State};

struct AppState {
    sessions: ssh::SessionMap,
}

#[tauri::command]
async fn ssh_connect(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    user: String,
    password: String,
    cols: u32,
    rows: u32,
) -> Result<u64, String> {
    ssh::connect(app, &state.sessions, host, port, user, password, cols, rows).await
}

#[tauri::command]
async fn ssh_write(state: State<'_, AppState>, id: u64, data: String) -> Result<(), String> {
    ssh::write(&state.sessions, id, &data).await
}

#[tauri::command]
async fn ssh_resize(state: State<'_, AppState>, id: u64, cols: u32, rows: u32) -> Result<(), String> {
    ssh::resize(&state.sessions, id, cols, rows).await
}

#[tauri::command]
async fn ssh_disconnect(state: State<'_, AppState>, id: u64) -> Result<(), String> {
    ssh::disconnect(&state.sessions, id).await
}

#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn main() {
    tauri::Builder::default()
        .manage(AppState { sessions: ssh::SessionMap::new() })
        .invoke_handler(tauri::generate_handler![
            ssh_connect,
            ssh_write,
            ssh_resize,
            ssh_disconnect,
            app_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
