/* EasySSH v4 (Tauri 2) — 应用入口 + IPC 命令（兼容 3.x preload.js API 语义） */
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod feedback;
mod key;
mod pf;
mod update_flow;
mod update_guard;
mod updater;
mod sftp;
mod ssh;
mod types;
mod xfer;

use parking_lot::Mutex;
use serde_json::Value;
use tauri::{Manager, State};

mod configstate;
use configstate::ConfigState;

struct AppState {
    sessions: ssh::SessionMap,
    config: ConfigState,
    sftps: sftp::SftpMap,
    xfers: xfer::XferState,
    pf: pf::PfState,
    upd: std::sync::Arc<update_flow::UpdateState>,
}

/* ---------- 版本 ---------- */

#[tauri::command]
fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/* ---------- 会话/设置持久化（config.json 兼容 3.x） ---------- */

#[tauri::command]
fn load_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.config.load_value()
}

#[tauri::command]
fn save_config(state: State<'_, AppState>, sessions: serde_json::Value, settings: serde_json::Value) -> Result<(), String> {
    state.config.save_value(sessions, settings)
}

/* ---------- 已知主机指纹 ---------- */

#[tauri::command]
fn knownhosts_get(state: State<'_, AppState>, host: String, port: u16) -> Option<String> {
    state.config.knownhosts_get(host, port)
}

#[tauri::command]
fn knownhosts_record(state: State<'_, AppState>, host: String, port: u16, fp: String) -> Result<(), String> {
    state.config.knownhosts_record(host, port, fp)
}

/* ---------- SSH 会话 ---------- */

#[tauri::command]
async fn ssh_connect(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    opts: types::ConnectOpts,
) -> Result<serde_json::Value, String> {
    let outcome = ssh::connect(app, &state.sessions, &opts).await?;
    Ok(serde_json::json!({ "id": outcome.id, "fingerprint": outcome.fingerprint }))
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
    // v4.3.1 健壮性：断开会话同时释放其 SFTP 通道（防跨会话串通道/泄漏）
    sftp::remove_session(&state.sftps, id);
    ssh::disconnect(&state.sessions, id).await
}

fn main() {
    // v5(M1)/F-M1-02：解析升级启动参数（须在 .bak 清理前——回滚依赖 .bak 存活）
    let (upgraded_ver, rollback_ver) = update_guard::parse_launch_args();
    update_guard::set_rollback_from(rollback_ver.clone());
    // v4.0.8:启动时清理在线升级遗留的旧版备份(EasySSH.exe.old / EasySSH.exe.bak)
    // v5(M1)/F-M1-02：`--upgraded` 启动时跳过 .bak 清理（15s 健康自检窗口内保留回滚材料），
    // 由看护线程在自检通过后统一清理；`--rollback` 启动（已回滚完成的旧版）同样跳过。
    let cfg_dir = configstate::default_base_dir();
    {
        let base = &cfg_dir;
        let skip_bak = upgraded_ver.is_some() || rollback_ver.is_some();
        for name in ["EasySSH.exe.old", "EasySSH.exe.bak", "EasySSH-3.x.exe.old"] {
            if skip_bak && name.ends_with(".bak") {
                continue;
            }
            let p = base.join(name);
            let _ = std::fs::remove_file(&p);
        }
    }
    // v4.2.4:UI 缓存强制清理(Rust 层,不依赖前端 JS 是否可执行)。
    // 背景:WebView2 可能持久缓存旧版 renderer.js,导致新修复(如 SFTP 右键)永远不生效;
    // 旧机制由前端 init() 触发清理,若旧 JS 本身有缺陷则清理永不发生。
    // 现在启动时(Rust,窗口创建前):
    //   1) 把 WebView2 用户数据目录固定到 <exe目录>\WebView2Data(由本程序接管,可安全删除)
    //   2) .ui-cache-ver 标记 = "版本|构建时间戳"——任何一次重新构建都与旧标记不同,
    //      不匹配即直接删除 WebView2 数据目录(缓存/旧 JS/旧 UI 彻底清零),再写入新标记。
    {
        let wv_data = cfg_dir.join("WebView2Data");
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &wv_data);
        let marker = cfg_dir.join(".ui-cache-ver");
        let expected = format!(
            "{}|{}",
            env!("CARGO_PKG_VERSION"),
            env!("EASYSSH_BUILD_ID")
        );
        let current = std::fs::read_to_string(&marker).unwrap_or_default();
        if current.trim() != expected {
            let _ = std::fs::remove_dir_all(&wv_data);
            let _ = std::fs::write(&marker, &expected);
        }
    }
    let config = ConfigState::new(cfg_dir.clone());
    // v5(M1)/F-M1-02：`--upgraded <版本>` 启动 → 启动升级健康看护线程（15s 健康自检窗口）
    if let Some(v) = upgraded_ver {
        update_guard::start_watchdog(v);
    }
    let sessions = ssh::SessionMap::new();
    let sftps = sftp::SftpMap::new();
    let xfers = xfer::XferState { sftps: sftps.clone(), tasks: Default::default(), cancels: Default::default(), gate: std::sync::Arc::new(tokio::sync::Semaphore::new(4)) };
    let pf = pf::PfState { sessions: sessions.clone(), listeners: Default::default() };
    let upd = std::sync::Arc::new(update_flow::UpdateState {
        base_dir: cfg_dir.clone(),
        last_result: Mutex::new(None),
        last_downloaded: Mutex::new(None),
        settings_api_base: Mutex::new(String::new()),
        sig_verified: Mutex::new(false),
        sig_legacy: Mutex::new(false),
    });

    tauri::Builder::default()
        // v5(M1)/F-M1-02：setup 完成即标记健康（Tauri 2 中主窗口在 setup 前已按配置创建，
        // 此处校验窗口存在性后写入健康标记，看护线程据此取消回滚并清理 .bak）
        .setup(|app| {
            let healthy = app.get_webview_window("main").is_some();
            if healthy {
                update_guard::mark_healthy();
            } else {
                eprintln!("[update-guard] 主窗口未创建，健康自检将失败并触发回滚");
            }
            // v4:补回 3.x 的静默自动检测规则（启动 3 秒 + 每小时），结果经事件推送渲染端显示升级横幅
            let state = app.state::<AppState>();
            update_flow::start_auto_checks(state.upd.clone(), app.handle().clone());
            Ok(())
        })
        // v4.3.1 健壮性：单实例——二次启动唤起既有窗口，避免双开写坏 config.json
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            sessions,
            config,
            sftps,
            xfers,
            pf,
            upd,
        })
        .register_uri_scheme_protocol("tauri", |ctx, request| {
            // v4.2.2 根治 WebView2 资源缓存:默认资源协议响应无 Cache-Control 头,
            // WebView2 启发式缓存导致原地升级后仍加载旧版 JS/CSS(修复"永不生效"总根因)。
            let raw = request.uri().path().trim_start_matches('/').to_string();
            let mut path = percent_decode(&raw);
            if path.is_empty() { path = "index.html".to_string(); }
            let mut built = tauri::http::Response::builder();
            match ctx.app_handle().asset_resolver().get(path.clone()) {
                Some(asset) => {
                    let bytes = asset.bytes().to_vec();
                    built = built
                        .status(tauri::http::StatusCode::OK)
                        .header("Content-Type", mime_of(&path))
                        .header("Cache-Control", "no-cache, no-store, must-revalidate")
                        .header("Pragma", "no-cache");
                    built.body(bytes).unwrap()
                }
                None => built.status(tauri::http::StatusCode::NOT_FOUND).body(Vec::new()).unwrap(),
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            load_config,
            save_config,
            knownhosts_get,
            knownhosts_record,
            ssh_connect,
            ssh_write,
            ssh_resize,
            ssh_disconnect,
            sftp_open,
            sftp_list,
            sftp_metadata,
            sftp_dirsize,
            sftp_rename,
            sftp_remove_file,
            sftp_remove_dir,
            sftp_mkdir,
            sftp_canonpath,
            xfer_start,
            xfer_cancel,
            xfer_list,
            xfer_download_dir,
            pick_save,
            pick_open,
            pf_start,
            pf_stop,
            pf_list,
            key_generate,
            key_install,
            key_diagnose,
            masterpw_status,
            masterpw_unlock,
            masterpw_enable,
            masterpw_change,
            masterpw_disable,
            masterpw_reset,
            update_check,
            fb_submit,
            update_check,
            update_last,
            update_pubkey_fp,
            rollback_notice,
            update_download,
            update_open_file,
            update_complete,
            save_text_file,
            read_text_file,
            cfg_dir_path,
            ui_cache_status,
            ui_cache_clear,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}



/* ---------- SFTP ---------- */

#[tauri::command]
async fn sftp_open(state: State<'_, AppState>, id: u64) -> Result<u64, String> {
    sftp::open(&state.sessions, &state.sftps, id).await
}

#[tauri::command]
async fn sftp_list(state: State<'_, AppState>, sftp_id: u64, dir: String) -> Result<Vec<serde_json::Value>, String> {
    sftp::list(&state.sftps, sftp_id, &dir).await
}

#[tauri::command]
async fn sftp_metadata(state: State<'_, AppState>, sftp_id: u64, path: String) -> Result<serde_json::Value, String> {
    sftp::metadata(&state.sftps, sftp_id, &path).await
}

// v4.3.2 新增:递归计算目录占用(防失控:深度40/条目20万上限,无权限目录跳过计数)
#[tauri::command]
async fn sftp_dirsize(state: State<'_, AppState>, sftp_id: u64, path: String) -> Result<serde_json::Value, String> {
    sftp::dir_size(&state.sftps, sftp_id, &path).await
}

#[tauri::command]
async fn sftp_rename(state: State<'_, AppState>, sftp_id: u64, old_path: String, new_path: String) -> Result<Value, String> {
    sftp::rename(&state.sftps, sftp_id, &old_path, &new_path).await
}

#[tauri::command]
async fn sftp_remove_file(state: State<'_, AppState>, sftp_id: u64, path: String) -> Result<Value, String> {
    sftp::remove_file(&state.sftps, sftp_id, &path).await
}

#[tauri::command]
async fn sftp_remove_dir(state: State<'_, AppState>, sftp_id: u64, path: String) -> Result<Value, String> {
    sftp::remove_dir(&state.sftps, sftp_id, &path).await
}

#[tauri::command]
async fn sftp_mkdir(state: State<'_, AppState>, sftp_id: u64, path: String) -> Result<Value, String> {
    sftp::mkdir(&state.sftps, sftp_id, &path).await
}

#[tauri::command]
async fn sftp_canonpath(state: State<'_, AppState>, sftp_id: u64) -> Result<String, String> {
    sftp::canonpath(&state.sftps, sftp_id).await
}

/* ---------- 传输队列 ---------- */

#[tauri::command]
async fn xfer_start(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    sftp_id: u64,
    direction: String,
    local_path: String,
    remote_path: String,
) -> Result<serde_json::Value, String> {
    xfer::start(state.xfers.clone(), app, direction, sftp_id, local_path, remote_path).await
}

#[tauri::command]
async fn xfer_cancel(state: State<'_, AppState>, task_id: u64) -> Result<Value, String> {
    xfer::cancel(state.xfers.clone(), task_id).await
}

#[tauri::command]
fn xfer_list(state: State<'_, AppState>) -> Vec<serde_json::Value> {
    xfer::list(&state.xfers)
}

#[tauri::command]
async fn xfer_download_dir(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    sftp_id: u64,
    remote_dir: String,
    local_dir: String,
) -> Result<u32, String> {
    xfer::download_dir(state.xfers.clone(), app, sftp_id, remote_dir, std::path::PathBuf::from(local_dir)).await
}

/* ---------- 文件选择对话框（tauri-plugin-dialog） ---------- */

use tauri_plugin_dialog::DialogExt;

#[tauri::command]
async fn pick_save(app: tauri::AppHandle, name: String) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(&name)
        .save_file(move |p| { let _ = tx.send(p.map(|x| x.to_string())); });
    rx.await.ok().flatten()
}

#[tauri::command]
fn cfg_dir_path() -> String {
    configstate::default_base_dir().to_string_lossy().to_string()
}

/// WebView2 旧缓存是否需要清理(版本号变更即需要;UI 修复生效的保障机制)
#[tauri::command]
fn ui_cache_status() -> bool {
    // v4.2.4:Rust 启动层已在窗口创建前完成比对与清理,正常情况下这里恒为 false;
    // 保留命令仅为兼容旧渲染端(JS 侧再触发一次 clear 无害)。
    let marker = configstate::default_base_dir().join(".ui-cache-ver");
    let expected = format!(
        "{}|{}",
        env!("CARGO_PKG_VERSION"),
        env!("EASYSSH_BUILD_ID")
    );
    std::fs::read_to_string(&marker)
        .map(|s| s.trim() != expected)
        .unwrap_or(true)
}

/// 清理 WebView2 全部浏览数据(缓存/旧 JS/CSS;自建分组与下载目录已由渲染端先备份到 ui-backup.json),
/// 完成后自动重载页面加载全新 UI
#[tauri::command]
fn ui_cache_clear(window: tauri::WebviewWindow) -> Result<(), String> {
    let marker = configstate::default_base_dir().join(".ui-cache-ver");
    let expected = format!(
        "{}|{}",
        env!("CARGO_PKG_VERSION"),
        env!("EASYSSH_BUILD_ID")
    );
    let _ = std::fs::write(&marker, expected);
    std::thread::spawn(move || {
        let _ = window.clear_all_browsing_data();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let _ = window.eval("location.reload();");
    });
    Ok(())
}

#[tauri::command]
async fn pick_open(app: tauri::AppHandle) -> Vec<String> {
    // 3.x 语义：返回路径数组（可多选）；渲染端按数组迭代
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .pick_files(move |paths| {
            let _ = tx.send(paths.map(|ps| ps.iter().map(|p| p.to_string()).collect()).unwrap_or_default());
        });
    rx.await.ok().unwrap_or_default()
}


/* ---------- 端口转发 ---------- */

#[tauri::command]
async fn pf_start(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: u64,
    cfg: pf::PfConfig,
) -> Result<String, String> {
    pf::start(state.pf.clone(), app, id, cfg).await
}

#[tauri::command]
fn pf_stop(state: State<'_, AppState>, id: u64, local_port: u16) -> Result<(), String> {
    pf::stop(&state.pf, id, local_port)
}

#[tauri::command]
fn pf_list(state: State<'_, AppState>, id: u64) -> Vec<serde_json::Value> {
    pf::list(&state.pf, id)
}


/* ---------- 密钥管理 ---------- */

#[tauri::command]
fn key_generate() -> Result<serde_json::Value, String> {
    let dir = configstate::default_base_dir();
    key::generate(&dir)
}

#[tauri::command]
async fn key_install(state: State<'_, AppState>, id: u64, pub_line: String) -> Result<String, String> {
    key::install(&state.sessions, id, &pub_line).await
}

#[tauri::command]
async fn key_diagnose(
    state: State<'_, AppState>,
    id: u64,
    pub_line: String,
) -> Result<String, String> {
    let dir = configstate::default_base_dir();
    key::diagnose(&state.sessions, &dir, id, &pub_line).await
}


/* ---------- 在线升级 ---------- */

#[tauri::command]
async fn update_check(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    // 同步 apiBase（渲染端设置里可改）
    if let Ok(cfg) = state.config.load_value() {
        if let Some(b) = cfg.get("settings").and_then(|s| s.get("apiBase")).and_then(|x| x.as_str()) {
            *state.upd.settings_api_base.lock() = b.to_string();
        }
    }
    update_flow::do_check(&state.upd, &app).await
}

#[tauri::command]
fn update_last(state: State<'_, AppState>) -> Option<serde_json::Value> {
    state.upd.last_result.lock().clone()
}

/// v5(M1)/F-M1-01：当前升级通道签名公钥指纹（设置页「检查更新」详情区展示）
#[tauri::command]
fn update_pubkey_fp() -> &'static str {
    updater::OTA_PUBKEY_FP
}

/// v5(M1)/F-M1-02：回滚提示——`--rollback <版本>` 启动时返回被回滚的新版版本号
#[tauri::command]
fn rollback_notice() -> Option<String> {
    update_guard::rollback_notice()
}

#[tauri::command]
async fn update_download(state: State<'_, AppState>, app: tauri::AppHandle, entry: serde_json::Value) -> Result<serde_json::Value, String> {
    update_flow::do_download(&state.upd, &app, &entry).await
}

#[tauri::command]
fn update_open_file(file: String) -> bool {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", file))
        .spawn();
    true
}

#[tauri::command]
fn update_complete(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    update_flow::store_app(app);
    update_flow::complete_upgrade(&state.upd)
}

/* ---------- 文件读写（导入导出对话框配套） ---------- */

#[tauri::command]
fn save_text_file(path: String, data: String) -> Result<(), String> {
    std::fs::write(&path, data).map_err(|e| format!("写入文件失败: {e}"))
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败: {e}"))
}


/* ---------- 使用反馈 ---------- */

#[tauri::command]
async fn fb_submit(state: State<'_, AppState>, payload: serde_json::Value) -> Result<serde_json::Value, String> {
    feedback::submit(&state.config.store.dir, payload).await
}


/* ---------- 主密码 ---------- */

#[tauri::command]
fn masterpw_status(state: State<'_, AppState>) -> serde_json::Value {
    state.config.masterpw_status()
}

#[tauri::command]
fn masterpw_unlock(state: State<'_, AppState>, pw: String) -> Result<serde_json::Value, String> {
    state.config.masterpw_unlock(&pw)?;
    Ok(serde_json::json!({ "ok": true, "message": "✅ 已解锁:会话密码可用" }))
}

#[tauri::command]
fn masterpw_enable(state: State<'_, AppState>, pw: String) -> Result<serde_json::Value, String> {
    state.config.masterpw_enable(&pw)?;
    Ok(serde_json::json!({ "ok": true, "message": "✅ 主密码已启用" }))
}

#[tauri::command]
fn masterpw_change(state: State<'_, AppState>, old_pw: String, new_pw: String) -> Result<serde_json::Value, String> {
    state.config.masterpw_change(&old_pw, &new_pw)?;
    Ok(serde_json::json!({ "ok": true, "message": "✅ 主密码已更改" }))
}

#[tauri::command]
fn masterpw_disable(state: State<'_, AppState>, pw: String) -> Result<serde_json::Value, String> {
    state.config.masterpw_disable(&pw)?;
    Ok(serde_json::json!({ "ok": true, "message": "✅ 主密码已关闭" }))
}

#[tauri::command]
fn masterpw_reset(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    state.config.masterpw_reset()?;
    Ok(serde_json::json!({ "ok": true, "message": "✅ 已重置" }))
}


// ================= v4.2.2 资源协议辅助 =================
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn mime_of(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}
