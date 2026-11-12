/* EasySSH v4：在线升级流程（对应 3.x updater.js 的 initUpdater 命令族）
 * 事件语义与 3.x 渲染端期望一致：update:event { event, payload }
 *   result / progress(download|verify) / downloaded / warning
 * 替换策略：原地改名 .bak → 新版顶替 → 分离重启；EBUSY 时退出后延迟替换兜底(cmd)。 */
use std::path::{Path, PathBuf};
use parking_lot::Mutex;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::updater;

pub struct UpdateState {
    pub base_dir: PathBuf,
    pub last_result: Mutex<Option<Value>>,
    pub last_downloaded: Mutex<Option<(PathBuf, String)>>, // (file, version)
    pub settings_api_base: Mutex<String>,
    /// v5(M1)/F-M1-01：本次升级清单的 Ed25519 验签是否通过（complete 阶段兜底复查）
    pub sig_verified: Mutex<bool>,
    /// 清单为 v4 老格式（无 ed25519 字段，兼容窗口放行）
    pub sig_legacy: Mutex<bool>,
    /// v5.1.2/R-20261111-001 Phase B：本轮 check_update 命中的升级主密钥
    /// （双密钥探测：新 master 优先 / master_prev 回退；下载解密必须用同一把）
    pub active_master: Mutex<Option<Vec<u8>>>,
}

fn emit_event(app: &AppHandle, event: &str, payload: Value) {
    let _ = app.emit("update:event", json!({ "event": event, "payload": payload }));
}

fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn api_base_of(state: &UpdateState) -> String {
    let b = state.settings_api_base.lock().clone();
    if b.is_empty() {
        "https://easyssh.cn".to_string()
    } else {
        b.trim_end_matches('/').to_string()
    }
}

/// 检查更新（返回 3.x 渲染端兼容的结果对象，并缓存为 last）
pub async fn do_check(state: &UpdateState, app: &AppHandle) -> Result<Value, String> {
    emit_event(app, "checking", json!({ "manual": true }));
    let masters = updater::load_masters(&state.base_dir)?;
    let base = api_base_of(state);
    let (manifest, master_idx) = updater::check_update(&base, &masters).await?;
    // 记录命中的密钥：本轮下载解密必须用同一把（双密钥轮换过渡窗口）
    *state.active_master.lock() = masters.get(master_idx).cloned();
    // v5(M1)/F-M1-01：清单（信封解密后）强制 Ed25519 验签——签名无效直接中止本轮升级
    // v5(M2)/任务二：验签失败在 stderr 之外持久化写入 cfgDir/update.log（M1 验收闭环）
    let (sig_verified, sig_legacy) = match updater::verify_manifest_signature(&manifest) {
        Ok(v) => v,
        Err(e) => {
            update_log(&state.base_dir, "VERIFY_FAILED", &e);
            return Err(e);
        }
    };
    *state.sig_verified.lock() = sig_verified;
    *state.sig_legacy.lock() = sig_legacy;
    let current = current_version();
    let latest = manifest
        .get("latestVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    // v5(M2)/任务二：检查成功关键节点
    update_log(
        &state.base_dir,
        "CHECK_OK",
        &format!(
            "latest={} current={} sigVerified={} sigLegacy={}",
            latest, current, sig_verified, sig_legacy
        ),
    );
    let eligible = updater::cmp_version(&latest, updater::MIN_UPGRADE_VERSION) != std::cmp::Ordering::Less
        && updater::cmp_version(&latest, &current) == std::cmp::Ordering::Greater;
    // 清单文件条目：kind=exe 或文件名 .exe（与 3.x 一致）
    let exe = (manifest.get("files").and_then(|f| f.as_array()))
        .and_then(|arr| {
            arr.iter()
                .find(|f| {
                    (f.get("kind").and_then(|k| k.as_str()) == Some("exe")
                        || f.get("file")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_lowercase().ends_with(".exe"))
                            .unwrap_or(false))
                        && f.get("encUrl").and_then(|x| x.as_str()).is_some()
                })
                .cloned()
        });

    let mut result = if latest.is_empty() {
        json!({ "status": "error", "message": "升级清单为空" })
    } else if eligible && exe.is_some() {
        let mut entry = exe.clone().unwrap();
        if let Some(o) = entry.as_object_mut() {
            o.insert("version".into(), json!(latest));
        }
        json!({
            "status": "available",
            "currentVersion": current,
            "latestVersion": latest,
            "releasedAt": manifest.get("releasedAt").cloned().unwrap_or(json!("")),
            "notes": manifest.get("notes").cloned().unwrap_or(json!([])),
            "file": entry,
            "checkedAt": chrono_utc(),
            "tempResident": false,
            "sigVerified": sig_verified,
            "sigLegacy": sig_legacy,
            "sigFp": updater::OTA_PUBKEY_FP,
        })
    } else {
        json!({ "status": "up-to-date", "currentVersion": current, "latestVersion": latest, "checkedAt": chrono_utc(), "tempResident": false })
    };
    if let Some(o) = result.as_object_mut() {
        o.insert("checkedAt".into(), json!(chrono_utc()));
    }
    *state.last_result.lock() = Some(result.clone());
    emit_event(app, "result", result.clone());
    Ok(result)
}

fn chrono_utc() -> String {
    // ISO 时间（简化实现，秒级）
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    super::key::iso_from_unix(secs)
}

/* ---------- v5(M2)/任务二：升级独立日志文件（M1 验收"验签失败记录升级日志"闭环） ---------- */
/* cfgDir 下 update.log：`[时间戳] 事件 详情` 一行一条；验签失败必记，其余关键节点
 * （检查成功/下载完成/替换完成）亦记录。保留策略：超过 200KB 时截掉前半旧内容。 */

/// 升级日志保留上限（字节）
const UPDATE_LOG_MAX_BYTES: u64 = 200 * 1024;

/// 追加一条升级日志（update.log）；任何 IO 失败静默（日志非关键路径）
pub fn update_log(base_dir: &Path, event: &str, detail: &str) {
    let log = base_dir.join("update.log");
    // 超限截断：保留后半（新内容），丢弃前半旧内容
    if let Ok(meta) = std::fs::metadata(&log) {
        if meta.len() > UPDATE_LOG_MAX_BYTES {
            if let Ok(old) = std::fs::read_to_string(&log) {
                let skip = old.chars().count() / 2;
                let kept: String = old.chars().skip(skip).collect();
                let _ = std::fs::write(&log, kept);
            }
        }
    }
    let line = format!("[{}] {} {}\n", chrono_utc(), event, detail);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 下载并校验（流式事件进度），成功后缓存 last_downloaded
pub async fn do_download(state: &UpdateState, app: &AppHandle, entry: &Value) -> Result<Value, String> {
    // v5(M1)/F-M1-01：下载前复查验签状态（清单在 do_check 解密并验签；未通过拒绝下载）
    if !*state.sig_verified.lock() {
        let e = "升级包签名校验失败，已中止安装（可到 easyssh.cn/download.html 手动下载并比对 SHA-256）";
        emit_event(app, "warning", json!({ "message": e }));
        return Err(e.to_string());
    }
    // v5.1.2/R-20261111-001 Phase B：优先用本轮 check_update 命中的密钥（与信封同源），
    // 未曾 check（异常路径）时退回主密钥
    let master = match state.active_master.lock().clone() {
        Some(m) => m,
        None => updater::load_master(&state.base_dir)?,
    };
    let base = api_base_of(state);
    let file = entry
        .get("file")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少文件名")?
        .to_string();
    let enc_url = entry
        .get("encUrl")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少加密包地址")?
        .to_string();
    let sha = entry
        .get("sha256")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少校验值")?
        .to_string();
    let total = entry.get("size").and_then(|x| x.as_u64()).unwrap_or(0);

    // v4.3.5 修复:复位事件改用独立类型 "downloading"——
    // 此前发空 payload 的 "downloaded" 会被渲染端误判为"下载完成",
    // 横幅提前进入「完成升级」阶段,用户点击时真实下载尚未完成(last_downloaded 为空)报"尚未下载"
    emit_event(app, "downloading", json!({ "phase": "start" }));
    let updates_dir = state.base_dir.join("updates");
    std::fs::create_dir_all(&updates_dir).map_err(|e| format!("创建 updates 目录失败: {e}"))?;
    let out = updates_dir.join(&file);
    let total_f = total as f64;
    let app2 = app.clone();
    let result = updater::download_and_decrypt(
        &base,
        &master,
        &enc_url,
        &file,
        &sha,
        &out,
        move |frac| {
            let pct = if total_f > 0.0 { frac } else { frac };
            emit_event(&app2, "progress", json!({ "phase": "download", "percent": (pct * 100.0) as u64 }));
        },
    )
    .await;
    if let Err(e) = result {
        emit_event(app, "warning", json!({ "message": e }));
        return Err(e);
    }
    // v4.3.9 修复:升级载荷文件名为 EasySSH-<ver>-update.exe,提取版本号时需剥掉 -update 后缀——
    // 此前未剥,version 变成 "4.3.8-update",升级后新文件被错误命名为 EasySSH-4.3.8-update-portable.exe
    let version = file
        .strip_prefix("EasySSH-")
        .and_then(|s| s.strip_suffix(".exe"))
        .and_then(|s| s.strip_suffix("-update"))
        .unwrap_or("")
        .to_string();
    *state.last_downloaded.lock() = Some((out.clone(), version.clone()));
    // v5(M2)/任务二：下载完成关键节点
    update_log(
        &state.base_dir,
        "DOWNLOAD_OK",
        &format!("file={} version={}", out.to_string_lossy(), version),
    );
    emit_event(
        app,
        "downloaded",
        json!({ "file": out.to_string_lossy(), "version": version }),
    );
    Ok(json!({ "ok": true, "file": out.to_string_lossy(), "version": version }))
}

/// 完成升级：原地改名替换 + 分离重启（EBUSY 走 cmd 延迟替换兜底）
pub fn complete_upgrade(state: &UpdateState) -> Result<Value, String> {
    let Some((new_file, version)) = state.last_downloaded.lock().clone() else {
        return Err("尚未下载新版本，请先下载并校验".into());
    };
    // v5(M1)/F-M1-01：替换（改 .bak / 顶替）前兜底复查清单验签状态——
    // 未通过验签的包绝不进入替换流程（解密后、替换前的最后防线）
    if !*state.sig_verified.lock() {
        // v5(M2)/任务二：complete 阶段验签兜底拦截同样写入升级日志
        let e = "升级包签名校验失败，已中止安装";
        update_log(&state.base_dir, "VERIFY_FAILED", &format!("complete 阶段兜底拦截: {e}"));
        return Err(e.into());
    }
    let current = current_version();
    if updater::cmp_version(&version, &current) != std::cmp::Ordering::Greater {
        return Err("目标版本不高于当前版本，拒绝替换（防降级）".into());
    }
    let exe = std::env::current_exe().map_err(|_| "无法定位当前 exe".to_string())?;
    let bak = exe.with_extension("exe.bak");
    let _ = std::fs::remove_file(&bak);

    // v4.3.7 命名方案（用户确认）：升级后新文件按新版本动态命名，旧版改名 .bak 回滚保留。
    //   旧: EasySSH-4.3.5-portable.exe → .bak
    //   新: EasySSH-4.3.6-portable.exe ( EasySSH-<新版本>-portable.exe )
    //   固定名用户(安装器解压的 EasySSH.exe)维持原地替换不变；
    //   同时自动添加/覆盖桌面与快速启动栏快捷方式指向新版文件。
    let exe_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let cur_name = exe.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let is_fixed_name = cur_name.eq_ignore_ascii_case("EasySSH.exe");
    let new_exe = if is_fixed_name {
        exe.clone() // 固定名用户：原地替换，行为不变
    } else {
        // 版本化命名：新文件名 = EasySSH-<新版本>-portable.exe（与官网下载位命名一致）
        exe_dir.join(format!("EasySSH-{}-portable.exe", version))
    };
    // v5.0.9/F-R9-01：桌面 + 快速启动栏 快捷方式同步刷新（新版路径），分离执行不阻塞升级。
    // v5.0.11 修复：移到此前的位置会把快捷方式指向「尚未落位」的新文件——一旦后续替换失败
    // （本次 5.0.10 现场事故），快捷方式立即失效且「Windows 找不到 \\」弹窗误导排查。
    // 现改为仅在【新版文件确认落位后】才刷新（原地成功路径见 placed 分支；延迟路径由脚本收尾刷新）。

    // 1) 尝试原地改名（Windows 允许对运行中的 exe 改名；被杀软/占用拦截时走兜底）
    // v5.0.11 修复：旧代码对「旧 exe→.bak」只试一次即转延迟替换，且失败原因不入日志。
    // 现改为 6×400ms 重试并把 OS 错误写入 update.log（本次 5.0.10 三次 deferred=true 的直接病灶）。
    let mut swapped = false;
    let mut swap_err = String::new();
    for i in 0..6 {
        match std::fs::rename(&exe, &bak) {
            Ok(_) => { swapped = true; break; }
            Err(e) => {
                swap_err = format!("{e} (os error {})", e.raw_os_error().unwrap_or(0));
                if i == 0 {
                    update_log(&state.base_dir, "BAK_RENAME_RETRY", &format!("旧 exe 改名被占用,重试中 err={swap_err}"));
                }
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
        }
    }
    if !swapped {
        update_log(&state.base_dir, "BAK_RENAME_FAILED", &swap_err);
    }
    let mut used_deferred = false;
    if swapped {
        // v5.0.8 加固：新下载的 exe 刚落盘常被杀软(360 等)短暂锁定扫描,单次 rename 直接失败
        // 导致「替换被拦截」误报。改为:rename 重试(6×400ms) → copy 兜底(源被只读共享锁时仍可读)
        // → 仍失败则回滚旧版并走延迟脚本兜底(等待期杀软扫描已完成)。
        let mut placed = false;
        for i in 0..6 {
            if std::fs::rename(&new_file, &new_exe).is_ok() { placed = true; break; }
            if i == 0 {
                update_log(&state.base_dir, "REPLACE_RETRY", &format!("rename 被占用,重试中 target={}", new_exe.to_string_lossy()));
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        if !placed {
            placed = std::fs::copy(&new_file, &new_exe).is_ok();
            if placed {
                let _ = std::fs::remove_file(&new_file); // copy 成功后清理 updates/ 内的载荷
            }
        }
        if placed {
            let _ = std::fs::remove_file(&new_file); // rename 成功时源已不在,忽略
            // v5.0.11：新版文件已确认落位，此时才刷新快捷方式（指向真实存在的新文件）
            refresh_shortcuts(&new_exe);
            // 2) v4.3.8 改进:等待旧进程完全退出后,再自动启动新版本
            //    (避免新旧实例因单实例机制冲突:新版被拒/旧版被聚焦)
            let old_pid = std::process::id();
            spawn_wait_then_start(old_pid, &new_exe, &version);
        } else {
            update_log(&state.base_dir, "REPLACE_DEFERRED", "rename/copy 均被锁定,回滚旧版并转延迟替换");
            let _ = std::fs::rename(&bak, &exe);
            used_deferred = true;
        }
    } else {
        used_deferred = true;
    }
    if used_deferred {
        // EBUSY 兜底：本进程退出释放锁后由分离脚本完成 备份→顶替→(超时兜底强杀旧进程)→启动新版→刷新快捷方式
        // v5.0.11 重写：旧实现为内联 cmd/ping 一行流（ping -n 10 & copy & move & taskkill & start）——
        //   5.0.10 现场三连败：整段脚本无逐步日志、`start` 路径异常时弹「Windows 找不到 \\」GUI 对话框、
        //   失败后无任何落位动作也无法事后追因。现改为独立 .ps1 走 -File 执行（免引号转义），
        //   每一步写入 update.log，任何失败可精确定位；不再使用 `start`（不弹 GUI 错误框）。
        let old_pid = std::process::id();
        match spawn_deferred_upgrade(state, old_pid, &exe, &bak, &new_file, &new_exe, &version) {
            Ok(_) => {}
            Err(e) => {
                update_log(&state.base_dir, "DEFERRED_SPAWN_FAILED", &e);
                return Err(format!("启动替换任务失败: {e}"));
            }
        }
    }
    let app_hold = APP_FOR_QUIT.lock().clone();
    if let Some(app) = app_hold {
        let a = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1200));
            a.exit(0);
        });
    }
    // v5(M2)/任务二：替换完成关键节点
    update_log(&state.base_dir, "COMPLETE_OK", &format!("version={} deferred={}", version, !swapped));
    Ok(json!({ "ok": true, "version": version, "deferred": !swapped }))
}

static APP_FOR_QUIT: Mutex<Option<AppHandle>> = Mutex::new(None);

pub fn store_app(app: AppHandle) {
    *APP_FOR_QUIT.lock() = Some(app);
}

/// 启动 3 秒后自动检测一次，之后每 1 小时检测一次（与 3.x 规则一致）。
/// 检测结果经 "update:event"(result) 事件推送，渲染端据此显示升级横幅。
pub fn start_auto_checks(state: std::sync::Arc<UpdateState>, app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Err(e) = do_check(&state, &app).await {
            eprintln!("update auto-check failed: {e}");
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            if let Err(e) = do_check(&state, &app).await {
                eprintln!("update auto-check failed: {e}");
            }
        }
    });
}

/// v4.3.10:等待旧进程完全退出后,自动启动新版本(隐藏窗口 bat,不阻塞升级流程)
/// 三重保障:1) 有界等待循环(最多约 30 秒) 2) 超时后 taskkill 强制结束旧进程
///          3) 确认旧进程消失后才 start 新版
/// 窗口抑制:cmd 仅用 CREATE_NO_WINDOW(此前 DETACHED_PROCESS|CREATE_NO_WINDOW 组合
/// 冲突,导致 ping/tasklist/find 各自弹出可见控制台窗口——v4.3.9 用户实测反馈)
fn spawn_wait_then_start(old_pid: u32, new_exe: &Path, version: &str) {
    let bat = std::env::temp_dir().join(format!("easyssh-upgrade-{}.cmd", old_pid));
    let script = format!(
        "@echo off\r\nrem wait for old process (pid={pid}) to exit, force-kill fallback, then start new version\r\nping -n 3 127.0.0.1 >nul\r\nset /a n=0\r\n:w\r\ntasklist /FI \"PID eq {pid}\" | find \"{pid}\" >nul\r\nif errorlevel 1 goto startnew\r\nset /a n=n+1\r\nif %n% GEQ 15 goto killold\r\nping -n 2 127.0.0.1 >nul\r\ngoto w\r\n:killold\r\ntaskkill /F /PID {pid} >nul 2>&1\r\nping -n 2 127.0.0.1 >nul\r\n:startnew\r\nstart \"\" \"{new}\" --upgraded {ver}\r\ndel \"%~f0\"\r\n",
        pid = old_pid,
        new = new_exe.display(),
        ver = version
    );
    if std::fs::write(&bat, script).is_err() {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", &bat.to_string_lossy()])
            .creation_flags(0x08000000) // 仅 CREATE_NO_WINDOW(修复可见控制台窗口问题)
            .spawn();
    }
}

/* ================= v5.0.9/F-R9-01 快捷方式生成（桌面 + 快速启动栏） =================
 * 需求变更（详见 docs/v5/需求变更记录池.md）：
 *   1) 新增「快速启动栏」快捷方式生成（任务栏固定区 User Pinned\TaskBar\EasySSH.lnk）；
 *   2) 两种快捷方式改为「设置 → 快捷方式」手动点击生成（shortcut_create 命令，同步执行并回传结果）；
 *   3) 升级完成流程保留自动同步：桌面 + 快速启动栏快捷方式一并刷新指向新版 exe。
 * 实现：PowerShell WScript.Shell 写 .lnk；手动路径同步等待并校验退出码，
 *       升级路径分离执行静默（快捷方式非关键路径，失败不影响升级）。 */

/// 快捷方式种类：桌面 / 快速启动栏
#[derive(Clone, Copy, PartialEq)]
pub enum ShortcutKind {
    Desktop,
    QuickLaunch,
}

impl ShortcutKind {
    fn parse(kind: &str) -> Option<Self> {
        match kind {
            "desktop" => Some(Self::Desktop),
            "quicklaunch" => Some(Self::QuickLaunch),
            _ => None,
        }
    }
    fn lnk_name(self) -> &'static str {
        match self {
            Self::Desktop | Self::QuickLaunch => "EasySSH.lnk",
        }
    }
    /// 目标目录的 PowerShell 表达式（含兜底建目录）
    fn dir_ps(self) -> &'static str {
        match self {
            Self::Desktop => "[Environment]::GetFolderPath('Desktop')",
            // 快速启动栏 = 任务栏固定区（Win10/11 通用路径）；目录缺失时兜底创建
            Self::QuickLaunch => "$ql=[Environment]::GetFolderPath('ApplicationData')+'\\Microsoft\\Internet Explorer\\Quick Launch\\User Pinned\\TaskBar'; if(!(Test-Path $ql)){New-Item -ItemType Directory -Path $ql -Force|Out-Null}; $ql",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Desktop => "桌面",
            Self::QuickLaunch => "快速启动栏",
        }
    }
}

/// 生成单条快捷方式（同步等待，返回可读结果；供设置页手动点击调用）
pub fn create_shortcut_sync(exe: &Path, kind: ShortcutKind) -> Result<String, String> {
    let exe_s = exe.to_string_lossy().replace('"', "");
    let workdir = exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let script = format!(
        "$d={dir}; $ws=New-Object -ComObject WScript.Shell; $s=$ws.CreateShortcut(\"$d\\{name}\"); $s.TargetPath=\"{exe}\"; $s.WorkingDirectory=\"{wd}\"; $s.IconLocation=\"{exe},0\"; $s.Save()",
        dir = kind.dir_ps(),
        name = kind.lnk_name(),
        exe = exe_s,
        wd = workdir,
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output()
            .map_err(|e| format!("启动 PowerShell 失败: {e}"))?;
        if out.status.success() {
            Ok(format!("✅ 已生成{}快捷方式: {}", kind.label(), kind.lnk_name()))
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("生成{}快捷方式失败: {}", kind.label(), err.trim()))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = script;
        Err("仅支持 Windows".into())
    }
}

/// 手动生成入口（设置页「快捷方式」）：解析种类并同步执行
pub fn shortcut_create(kind_str: &str) -> Result<String, String> {
    let kind = ShortcutKind::parse(kind_str).ok_or("未知快捷方式种类（desktop/quicklaunch）")?;
    let exe = std::env::current_exe().map_err(|_| "无法定位当前 exe".to_string())?;
    create_shortcut_sync(&exe, kind)
}

/// v5.0.9：升级完成后自动同步刷新 桌面 + 快速启动栏 快捷方式指向新版文件
/// （分离执行 PowerShell WScript.Shell，失败静默——快捷方式非关键路径）
fn refresh_shortcuts(new_exe: &Path) {
    for kind in [ShortcutKind::Desktop, ShortcutKind::QuickLaunch] {
        let exe_s = new_exe.to_string_lossy().replace('"', "");
        let workdir = new_exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let script = format!(
            "$d={dir}; $ws=New-Object -ComObject WScript.Shell; $s=$ws.CreateShortcut(\"$d\\{name}\"); $s.TargetPath=\"{exe}\"; $s.WorkingDirectory=\"{wd}\"; $s.IconLocation=\"{exe},0\"; $s.Save()",
            dir = kind.dir_ps(),
            name = kind.lnk_name(),
            exe = exe_s,
            wd = workdir,
        );
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();
        }
    }
}

/* v5.0.11：延迟替换兜底（PowerShell -File 版，替代旧 cmd/ping 一行流）
 * 时序：等旧进程退出(≤30s，超时强杀) → 旧 exe 改名 .bak(重试) → 载荷 move 到新版正式名(重试)
 *      → 分离启动新版(--upgraded <ver>) → 刷新桌面/快速启动栏快捷方式 → 全程逐步写 update.log */
fn spawn_deferred_upgrade(
    state: &UpdateState,
    old_pid: u32,
    exe: &std::path::Path,
    bak: &std::path::Path,
    new_file: &std::path::Path,
    new_exe: &std::path::Path,
    version: &str,
) -> Result<(), String> {
    // PowerShell 单引号转义：内部 ' 翻倍（Windows 路径几乎不会出现，防御性处理）
    fn psq(p: &std::path::Path) -> String {
        format!("'{}'", p.to_string_lossy().replace('\'', "''"))
    }
    let qver = format!("'{}'", version.replace('\'', "''"));
    let log_q = psq(&state.base_dir.join("update.log"));
    let desktop_dir = ShortcutKind::Desktop.dir_ps();   // PS 表达式（如 $env:USERPROFILE\Desktop），保持原样内插
    let qlaunch_dir = ShortcutKind::QuickLaunch.dir_ps();
    let lnk = ShortcutKind::Desktop.lnk_name();
    let exe_q = psq(exe);
    let bak_q = psq(bak);
    let bak_name = bak.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default().replace('\'', "''");
    let new_file_q = psq(new_file);
    let new_exe_q = psq(new_exe);

    let script = format!(
r#"$ErrorActionPreference = 'Continue'
$log = {log_q}
function Log($m) {{ Add-Content -LiteralPath $log -Value ('- ' + (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd\THH:mm:ss\Z') + ' [deferred] ' + $m) -Encoding UTF8 }}
Log ('START old_pid={old_pid} target=' + {new_exe_q})
# 1) 等待旧进程退出（最多 30 秒；之后强杀兜底）
$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline -and (Get-Process -Id {old_pid} -ErrorAction SilentlyContinue)) {{ Start-Sleep -Milliseconds 500 }}
if (Get-Process -Id {old_pid} -ErrorAction SilentlyContinue) {{ Log 'OLDPID_TIMEOUT_TASKKILL'; Stop-Process -Id {old_pid} -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 1 }}
Log 'OLD_EXITED'
# 2) 旧 exe 改名 .bak（重试 20×500ms；已存在则跳过）
for ($i = 0; $i -lt 20; $i++) {{
  if (-not (Test-Path -LiteralPath {exe_q})) {{ break }}
  try {{ Rename-Item -LiteralPath {exe_q} -NewName '{bak_name}' -ErrorAction Stop; break }}
  catch {{ if ($i -eq 0) {{ Log ('BAK_RENAME_RETRY ' + $_.Exception.Message) }}; Start-Sleep -Milliseconds 500 }}
}}
# 3) 载荷 move 到新版正式名（重试 20×500ms）
$placed = $false
for ($i = 0; $i -lt 20; $i++) {{
  try {{ Move-Item -LiteralPath {new_file_q} -Destination {new_exe_q} -ErrorAction Stop; $placed = $true; break }}
  catch {{ if ($i -eq 0) {{ Log ('MOVE_RETRY ' + $_.Exception.Message) }}; Start-Sleep -Milliseconds 500 }}
}}
if (-not $placed) {{ Log 'DEFERRED_MOVE_FAILED'; exit 1 }}
Log 'DEFERRED_PLACED'
# 4) 分离启动新版（不用 start，ShellExecute 不弹 GUI 错误框；失败写日志）
try {{ Start-Process -FilePath {new_exe_q} -ArgumentList @('--upgraded',{qver}) -WorkingDirectory (Split-Path -Parent {new_exe_q}) -ErrorAction Stop; Log 'DEFERRED_STARTED' }}
catch {{ Log ('DEFERRED_START_FAILED ' + $_.Exception.Message); exit 1 }}
# 5) 刷新桌面 + 快速启动栏快捷方式（失败静默，非关键路径）
try {{
  $ws = New-Object -ComObject WScript.Shell
  foreach ($d in @({desktop_dir},{qlaunch_dir})) {{
    try {{
      $s = $ws.CreateShortcut(($d + '\{lnk}'))
      $s.TargetPath = {new_exe_q}; $s.WorkingDirectory = (Split-Path -Parent {new_exe_q}); $s.IconLocation = ({new_exe_q} + ',0'); $s.Save()
    }} catch {{ Log ('LNK_FAILED ' + $_.Exception.Message) }}
  }}
  Log 'LNK_REFRESHED'
}} catch {{ Log ('LNK_FAILED ' + $_.Exception.Message) }}
Log 'DEFERRED_DONE'
"#,
        log_q = log_q,
        old_pid = old_pid,
        new_exe_q = new_exe_q,
        exe_q = exe_q,
        bak_name = bak_name,
        new_file_q = new_file_q,
        qver = qver,
        desktop_dir = desktop_dir,
        qlaunch_dir = qlaunch_dir,
        lnk = lnk,
    );

    // 脚本落盘 updates/upgrade-deferred.ps1（每次覆盖）
    let updates_dir = state.base_dir.join("updates");
    std::fs::create_dir_all(&updates_dir).map_err(|e| format!("创建 updates 目录失败: {e}"))?;
    let ps_path = updates_dir.join("upgrade-deferred.ps1");
    std::fs::write(&ps_path, &script).map_err(|e| format!("写入延迟替换脚本失败: {e}"))?;

    update_log(&state.base_dir, "DEFERRED_SCRIPTED", &format!("script={}", ps_path.to_string_lossy()));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&ps_path)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("{e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (old_pid, exe, bak, new_file, new_exe, version);
        Err("仅支持 Windows".into())
    }
}
