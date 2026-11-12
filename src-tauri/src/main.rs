/* EasySSH v4 (Tauri 2) — 应用入口 + IPC 命令（兼容 3.x preload.js API 语义） */
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod feedback;
mod geo;
mod key;
mod net;
mod pf;
mod update_flow;
mod update_guard;
mod updater;
mod sftp;
mod ssh;
mod stats;
mod types;
mod xfer;
mod xfer_core;

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
    let outcome = ssh::connect(app, &state.sessions, &state.config, &opts).await?;
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

/* ---------- v5(M2)/F-M2-06：panic hook + 崩溃日志文件 ---------- */
/* 自实现（不引新依赖）：panic 时写入 cfgDir 下 crash-<时间戳>.log——
 *   panic 信息 / 位置 / 线程 / 版本号 / 简单回溯（≤20 帧）；不含任何会话/密码数据。
 * 目录下保留最近 5 个，写入后自动清理更旧的。
 * 手册验收（F-M2-06）：注入 panic → logs 生成含时间戳、版本、线程、堆栈的日志文件。 */

/// ISO 时间戳（复用 key::iso_from_unix，秒级）
fn crashlog_iso_ts() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    key::iso_from_unix(secs)
}

/// 清理 crash 日志：仅保留最新 5 个（文件名含毫秒时间戳，字典序即时间序）
fn cleanup_crash_logs(dir: &std::path::Path) {
    const KEEP: usize = 5;
    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("crash-") && n.to_string_lossy().ends_with(".log"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => return,
    };
    entries.sort(); // crash-<毫秒>.log 定长时间戳 → 字典序 = 时间序
    if entries.len() > KEEP {
        for p in entries[..entries.len() - KEEP].iter() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// v5(beta)/崩溃日志查看：名称白名单校验（crash-<纯数字>.log），防路径穿越
fn crash_log_path(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let ok = name.starts_with("crash-")
        && name.ends_with(".log")
        && name["crash-".len()..name.len() - 4].chars().all(|c| c.is_ascii_digit());
    if !ok {
        return None;
    }
    Some(dir.join(name))
}

/// v5(beta)：列出崩溃日志（名称/大小/修改时间，按时间倒序）
#[tauri::command]
fn crash_logs() -> Vec<serde_json::Value> {
    let mut out: Vec<serde_json::Value> = Vec::new();
    { let cfg = configstate::default_base_dir();
        if let Ok(rd) = std::fs::read_dir(&cfg) {
            for e in rd.filter_map(|e| e.ok()) {
                let n = e.file_name().to_string_lossy().to_string();
                if !(n.starts_with("crash-") && n.ends_with(".log")) {
                    continue;
                }
                let md = e.metadata().ok();
                let mtime = md
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let d: std::time::Duration = std::time::SystemTime::now()
                            .duration_since(t)
                            .unwrap_or_default();
                        format!("{}秒前", d.as_secs())
                    });
                out.push(serde_json::json!({
                    "name": n,
                    "size": md.as_ref().map(|m| m.len()),
                    "mtime": mtime,
                }));
            }
        }
    }
    out.sort_by(|a, b| {
        let na = a["name"].as_str().unwrap_or("");
        let nb = b["name"].as_str().unwrap_or("");
        nb.cmp(na) // 倒序:最新在前
    });
    out
}

/// v5(beta)：读取单个崩溃日志（前 4000 字符，与前端展示上限一致）
#[tauri::command]
fn crash_log_read(name: String) -> Result<String, String> {
    let cfg = configstate::default_base_dir();
    let p = crash_log_path(&cfg, &name).ok_or("非法的日志文件名")?;
    std::fs::read(&p)
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .map_err(|e| format!("读取失败: {e}"))
}

/// v5(beta)：删除单个崩溃日志
#[tauri::command]
fn crash_log_delete(name: String) -> Result<(), String> {
    let cfg = configstate::default_base_dir();
    let p = crash_log_path(&cfg, &name).ok_or("非法的日志文件名")?;
    std::fs::remove_file(&p).map_err(|e| format!("删除失败: {e}"))
}

/// panic 信息提取：支持 &str 与 String 两种 payload（不含用户数据）
fn panic_payload_msg(info: &std::panic::PanicHookInfo) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "<非字符串 panic payload>".to_string()
    }
}

/// 安装 panic hook（main 启动最早期调用，须在其他初始化之前）
fn install_panic_hook() {
    let dir = configstate::default_base_dir();
    std::panic::set_hook(Box::new(move |info| {
        let msg = panic_payload_msg(info);
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<未知位置>".to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        // 简单回溯：截取前 20 帧
        let bt_full = std::backtrace::Backtrace::force_capture().to_string();
        let bt_20: Vec<&str> = bt_full.lines().take(20).collect();
        let content = format!(
            "EasySSH v{}\n时间: {}\n线程: {}\npanic: {}\n位置: {}\n\n回溯(前 {} 帧):\n{}\n",
            env!("CARGO_PKG_VERSION"),
            crashlog_iso_ts(),
            thread,
            msg,
            loc,
            bt_20.len(),
            bt_20.join("\n")
        );
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let file = dir.join(format!("crash-{}.log", ts_ms));
        let _ = std::fs::write(&file, content);
        cleanup_crash_logs(&dir);
        // 控制台同步输出（便携模式下便于现场定位）
        eprintln!("[crash] panic @ {} 线程 {}——已写入 {}", loc, thread, file.display());
    }));
}

fn main() {
    // v5(M2)/F-M2-06：panic hook 接线（启动最早期，覆盖后续一切初始化/运行期 panic）
    install_panic_hook();
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
            // v5(beta) 优化:不再同步 remove_dir_all(大目录删除+360 扫描曾拖慢启动 10 秒+)——
            // 改为原子改名旧目录(立即得到全新 WebView2Data),旧目录交后台线程慢慢删除
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let stale = cfg_dir.join(format!("WebView2Data-old-{}", stamp));
            if std::fs::rename(&wv_data, &stale).is_ok() {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let _ = std::fs::remove_dir_all(&stale);
                });
            } else {
                let _ = std::fs::remove_dir_all(&wv_data);
            }
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
    // v5(M4)/F-M4-02：并发闸门升级为优先级调度器（capacity=4 语义不变，槽位由调度器仲裁）
    let (disp_tx, disp_rx) = tokio::sync::mpsc::unbounded_channel();
    let xfers = xfer::XferState {
        sftps: sftps.clone(),
        tasks: Default::default(),
        cancels: Default::default(),
        pauses: Default::default(),
        starts: Default::default(),
        disp: xfer::ArcDisp(std::sync::Arc::new(parking_lot::Mutex::new(xfer::DispInner {
            tx: disp_tx,
            rx: Some(disp_rx),
        }))),
        disp_started: Default::default(),
        capacity: 4,
        range_done: Default::default(),
    };
    let pf = pf::PfState { sessions: sessions.clone(), listeners: Default::default() };
    let upd = std::sync::Arc::new(update_flow::UpdateState {
        base_dir: cfg_dir.clone(),
        last_result: Mutex::new(None),
        last_downloaded: Mutex::new(None),
        settings_api_base: Mutex::new(String::new()),
        sig_verified: Mutex::new(false),
        sig_legacy: Mutex::new(false),
    });

    // v5.0.8:`--smoke` 启动参数 → 跳过单实例注册(发版冒烟门禁在本机已有 EasySSH 运行时,
    // 新 exe 会因单实例机制立即 exit(0),导致门禁误报"8 秒内退出")
    let smoke_mode = std::env::args().any(|a| a == "--smoke");
    let builder = tauri::Builder::default();
    let builder = if smoke_mode {
        builder
    } else {
        builder
            // v4.3.1 健壮性：单实例——二次启动唤起既有窗口，避免双开写坏 config.json
            .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }))
    };
    builder
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
            // v5.1.0:客户端匿名使用统计(默认开启,设置-升级-隐私可关闭)——
            // 延迟 90 秒后台执行,失败静默,10 分钟后重试一次,不影响启动
            {
                let base = state.config.store.dir.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(90)).await;
                    if let Err(e) = stats::maybe_report(&base).await {
                        // 关闭/去抖属正常跳过;网络失败 10 分钟后重试一次
                        if !(e.contains("已关闭") || e.contains("已上报")) {
                            tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                            let _ = stats::maybe_report(&base).await;
                        }
                    }
                });
            }
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
            xfer_pause,
            xfer_resume,
            xfer_list,
            xfer_download_dir,
            pick_save,
            pick_open,
            external_sessions_read,
            pf_start,
            pf_stop,
            pf_list,
            pf_forwards_save,
            pf_forwards_get,
            pf_apply_session,
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
            shortcut_create,
            save_text_file,
            read_text_file,
            cfg_dir_path,
            crash_logs,
            crash_log_read,
            crash_log_delete,
            ui_cache_status,
            ui_cache_clear,
            ip_geo_lookup,
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
    // v5(M2)/F-M2-01 + F-M2-02：并行管道 worker 数（默认 8）与断点续传开关
    pipeline: Option<usize>,
    resume: Option<bool>,
    // v5(M4)/F-M4-02：任务优先级 'high'|'normal'|'low'（默认 normal，高优先先分配并发槽位）
    priority: Option<String>,
    // v5(M4.1)/F-M4-02：单任务限速（字节/秒，0 或缺省=不限速，按 worker 数均分块间节流）
    speed_limit: Option<u64>,
) -> Result<serde_json::Value, String> {
    xfer::start(state.xfers.clone(), app, direction, sftp_id, local_path, remote_path, pipeline, resume, priority, speed_limit).await
}

#[tauri::command]
async fn xfer_cancel(state: State<'_, AppState>, task_id: u64) -> Result<Value, String> {
    xfer::cancel(state.xfers.clone(), task_id).await
}

/// v5(M4)/F-M4-02：暂停传输任务（保留断点，任务保活）
#[tauri::command]
fn xfer_pause(state: State<'_, AppState>, app: tauri::AppHandle, task_id: u64) -> Result<Value, String> {
    xfer::pause(&state.xfers, &app, task_id)
}

/// v5(M4)/F-M4-02：恢复已暂停任务（从暂停处继续）
#[tauri::command]
fn xfer_resume(state: State<'_, AppState>, app: tauri::AppHandle, task_id: u64) -> Result<Value, String> {
    xfer::resume(&state.xfers, &app, task_id)
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

/* ---------- IP 归属地主备查询(v5.1.1/R-20261111-004) ---------- */

/// ip 传 IPv4/IPv6 地址；不传则查询本机公网出口。
/// 主 ip.263.tw 加密API → 备① 百度(v4) → 备② ip-api.com(v4+v6)，全失败静默返回 Err。
#[tauri::command]
async fn ip_geo_lookup(ip: Option<String>) -> Result<Value, String> {
    geo::lookup(&configstate::default_base_dir(), ip).await
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

/* ---------- v5(M4)/F-M4-07：会话转发规则持久化 + 一键应用 ---------- */

/// 保存会话转发规则集（session.forwards 字段；覆盖式）
#[tauri::command]
fn pf_forwards_save(
    state: State<'_, AppState>,
    session_key: String,
    forwards: Vec<serde_json::Value>,
) -> Result<(), String> {
    state.config.session_forwards_set(&session_key, forwards)
}

/// 读取会话转发规则集（旧会话无 forwards 字段 = 空数组 = 无规则）
#[tauri::command]
fn pf_forwards_get(state: State<'_, AppState>, session_key: String) -> Vec<serde_json::Value> {
    state.config.session_forwards_get(&session_key)
}

/// 连接建立后一键应用该会话的全部转发规则（批量启动；已监听端口跳过）。
/// 返回 { started, skipped, errors }，errors 为 [端口, 原因] 对。
#[tauri::command]
async fn pf_apply_session(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: u64,
    session_key: String,
) -> Result<serde_json::Value, String> {
    let forwards = state.config.session_forwards_get(&session_key);
    let mut started = 0u32;
    let mut skipped = 0u32;
    let mut errors: Vec<Value> = Vec::new();
    for f in forwards {
        let lport = f.get("localPort").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
        let cfg: pf::PfConfig = match serde_json::from_value(f.clone()) {
            Ok(c) => c,
            Err(e) => {
                errors.push(serde_json::json!({ "localPort": lport, "error": format!("规则格式错误: {e}") }));
                continue;
            }
        };
        match pf::start(state.pf.clone(), app.clone(), id, cfg).await {
            Ok(_) => started += 1,
            Err(e) if e.contains("已在监听") || e.contains("已在运行") => skipped += 1,
            Err(e) => errors.push(serde_json::json!({ "localPort": lport, "error": e })),
        }
    }
    Ok(serde_json::json!({ "ok": true, "started": started, "skipped": skipped, "errors": errors }))
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

/// v5.0.9/F-R9-01：设置页手动生成快捷方式（desktop=桌面 / quicklaunch=快速启动栏）
#[tauri::command]
fn shortcut_create(kind: String) -> Result<String, String> {
    update_flow::shortcut_create(&kind)
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

/* ---------- v5.0.8/R-003 外部会话导入数据读取 ---------- */

/// 解析 `reg query HKCU\...\PuTTY\Sessions /s` 输出为会话条目(仅 SSH;纯函数便于单测)
fn parse_putty_sessions(text: &str) -> Vec<serde_json::Value> {
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let (mut cur_host, mut cur_port, mut cur_user, mut cur_proto, mut cur_name) =
        (String::new(), String::new(), String::new(), String::new(), String::new());
    let mut flush = |name: &str, host: &str, port: &str, user: &str, proto: &str, entries: &mut Vec<serde_json::Value>| {
        if host.trim().is_empty() { return; }
        if !proto.is_empty() && !proto.eq_ignore_ascii_case("ssh") { return; }
        let port_v: u16 = {
            let t = port.trim();
            // PuTTY 端口可能存为十六进制(0x...)或十进制
            let n = if let Some(h) = t.strip_prefix("0x") { u32::from_str_radix(h, 16).unwrap_or(22) } else { t.parse::<u32>().unwrap_or(22) };
            n.clamp(1, 65535) as u16
        };
        // 会话名:注册表键名 URL 编码(%20 等)解码为可读名
        let decoded = name.split('\\').next_back().unwrap_or(name).replace("%20", " ").replace("%25", "%");
        entries.push(serde_json::json!({
            "name": if decoded.is_empty() { host.trim() } else { &decoded },
            "host": host.trim(),
            "port": port_v,
            "username": user.trim(),
        }));
    };
    for line in text.lines() {
        let t = line.trim_end();
        if t.starts_with("HKEY_") {
            flush(&cur_name, &cur_host, &cur_port, &cur_user, &cur_proto, &mut entries);
            cur_host.clear(); cur_port.clear(); cur_user.clear(); cur_proto.clear();
            cur_name = t.to_string();
            continue;
        }
        // 值行:名称 + 类型 + 值(值可含空格,取类型列之后的整段)
        let tt = t.trim();
        let mut ws = tt.split_whitespace();
        let (Some(k), Some(vt)) = (ws.next(), ws.next()) else { continue };
        let vt_up = vt.to_ascii_uppercase();
        if vt_up != "REG_SZ" && vt_up != "REG_DWORD" { continue; }
        // 值 = 类型列之后的剩余整段(reg 输出用对齐空格分隔,不能按空白切分值)
        let val = match tt.find(vt) {
            Some(i) => tt[i + vt.len()..].trim().to_string(),
            None => String::new(),
        };
        match k.to_ascii_lowercase().as_str() {
            "hostname" => cur_host = val,
            "portnumber" => cur_port = val,
            "username" => cur_user = val,
            "protocol" => cur_proto = val,
            _ => {}
        }
    }
    flush(&cur_name, &cur_host, &cur_port, &cur_user, &cur_proto, &mut entries);
    entries
}

#[cfg(test)]
mod ext_session_tests {
    use super::parse_putty_sessions;

    const SAMPLE: &str = "\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\r\n\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\prod%20web\r\n    HostName    REG_SZ    10.0.0.1\r\n    PortNumber    REG_DWORD    0x000008ae\r\n    UserName    REG_SZ    deploy\r\n    Protocol    REG_SZ    ssh\r\n\r\nHKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\serial%25box\r\n    HostName    REG_SZ    10.0.0.2\r\n    PortNumber    REG_SZ    2200\r\n    UserName    REG_SZ    root\r\n    Protocol    REG_SZ    telnet\r\n";

    #[test]
    fn t01_parse_putty_sample() {
        let es = parse_putty_sessions(SAMPLE);
        assert_eq!(es.len(), 1, "telnet 会话应被过滤: {es:?}");
        let e = &es[0];
        assert_eq!(e["name"], "prod web");
        assert_eq!(e["host"], "10.0.0.1");
        assert_eq!(e["port"], 2222); // 0x8ae
        assert_eq!(e["username"], "deploy");
    }

    #[test]
    fn t02_empty_and_defaults() {
        assert!(parse_putty_sessions("").is_empty());
        assert!(parse_putty_sessions("HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\r\n").is_empty());
        // 无端口/无协议 → 默认 22 并保留
        let es = parse_putty_sessions("HKEY_CURRENT_USER\\x\\a\r\n    HostName    REG_SZ    1.2.3.4\r\n");
        assert_eq!(es.len(), 1);
        assert_eq!(es[0]["port"], 22);
        assert_eq!(es[0]["name"], "a");
    }
}


/// 读取外部会话来源的原始数据(解析在渲染端 JS 完成):
/// - putty  : 枚举注册表 HKCU\Software\SimonTatham\PuTTY\Sessions(经 reg query,不引新依赖)
///            返回 { entries: [{ name, host, port, username }] }(仅 SSH 协议)
/// - xshell : 自动发现 文档\NetSarang Computer\*\Xshell\Sessions(含旧版 Documents\Xshell\Sessions)
///            返回 { files: [{ name, text }] }(.xsh 为 XML,渲染端 DOMParser 解析)
#[tauri::command]
fn external_sessions_read(source: String) -> Result<serde_json::Value, String> {
    use std::os::windows::process::CommandExt; // creation_flags(隐藏 reg 命令行窗口)
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    match source.as_str() {
        "putty" => {
            let out = std::process::Command::new("reg")
                .args(["query", r"HKCU\Software\SimonTatham\PuTTY\Sessions", "/s"])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .map_err(|e| format!("读取注册表失败: {e}"))?;
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            Ok(serde_json::json!({ "ok": true, "entries": parse_putty_sessions(&text) }))
        }
        "xshell" => {
            let profile = std::env::var("USERPROFILE").map_err(|_| "无法获取用户目录".to_string())?;
            let docs = std::path::Path::new(&profile).join("Documents");
            // 候选目录:Xshell 6/7/8(NetSarang Computer\版本号\Xshell\Sessions)与旧版布局
            let mut roots: Vec<std::path::PathBuf> = Vec::new();
            for sub in ["NetSarang Computer", "NetSarang", "NetSarang Computer"] {
                let base = docs.join(sub);
                if sub == "NetSarang Computer" {
                    if let Ok(rd) = std::fs::read_dir(&base) {
                        for e in rd.flatten() {
                            let p = e.path().join("Xshell").join("Sessions");
                            if p.is_dir() { roots.push(p); }
                        }
                    }
                } else if let Ok(rd) = std::fs::read_dir(&base) {
                    for e in rd.flatten() {
                        let p = e.path().join("Xshell").join("Sessions");
                        if p.is_dir() { roots.push(p); }
                    }
                }
            }
            roots.push(docs.join("Xshell").join("Sessions"));
            let mut files: Vec<serde_json::Value> = Vec::new();
            let mut stack: Vec<std::path::PathBuf> = roots;
            let mut visited = 0usize;
            while let Some(dir) = stack.pop() {
                visited += 1;
                if visited > 200 { break; }
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().map(|x| x.eq_ignore_ascii_case("xsh")).unwrap_or(false) {
                        if files.len() >= 500 { break; }
                        if let Ok(text) = std::fs::read_to_string(&p) {
                            let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                            files.push(serde_json::json!({ "name": name, "text": text }));
                        }
                    }
                }
            }
            Ok(serde_json::json!({ "ok": true, "files": files }))
        }
        _ => Err(format!("未知来源: {source}")),
    }
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
