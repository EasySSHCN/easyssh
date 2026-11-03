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
    let master = updater::load_master(&state.base_dir)?;
    let base = api_base_of(state);
    let manifest = updater::check_update(&base, &master).await?;
    // v5(M1)/F-M1-01：清单（信封解密后）强制 Ed25519 验签——签名无效直接中止本轮升级
    let (sig_verified, sig_legacy) = updater::verify_manifest_signature(&manifest)?;
    *state.sig_verified.lock() = sig_verified;
    *state.sig_legacy.lock() = sig_legacy;
    let current = current_version();
    let latest = manifest
        .get("latestVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
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

/// 下载并校验（流式事件进度），成功后缓存 last_downloaded
pub async fn do_download(state: &UpdateState, app: &AppHandle, entry: &Value) -> Result<Value, String> {
    // v5(M1)/F-M1-01：下载前复查验签状态（清单在 do_check 解密并验签；未通过拒绝下载）
    if !*state.sig_verified.lock() {
        let e = "升级包签名校验失败，已中止安装（可到 easyssh.cn/download.html 手动下载并比对 SHA-256）";
        emit_event(app, "warning", json!({ "message": e }));
        return Err(e.to_string());
    }
    let master = updater::load_master(&state.base_dir)?;
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
        return Err("升级包签名校验失败，已中止安装".into());
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
    //   同时自动添加/覆盖桌面快捷方式指向新版文件。
    let exe_dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let cur_name = exe.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let is_fixed_name = cur_name.eq_ignore_ascii_case("EasySSH.exe");
    let new_exe = if is_fixed_name {
        exe.clone() // 固定名用户：原地替换，行为不变
    } else {
        // 版本化命名：新文件名 = EasySSH-<新版本>-portable.exe（与官网下载位命名一致）
        exe_dir.join(format!("EasySSH-{}-portable.exe", version))
    };
    // 桌面快捷方式刷新（新版路径），分离执行不阻塞升级
    refresh_desktop_shortcut(&new_exe);

    // 1) 尝试原地改名（Windows 允许对运行中的 exe 改名；被杀软/占用拦截时走兜底）
    let swapped = std::fs::rename(&exe, &bak).is_ok();
    if swapped {
        if std::fs::rename(&new_file, &new_exe).is_err() {
            let _ = std::fs::rename(&bak, &exe);
            return Err("替换被系统/杀软拦截，已自动恢复旧版本".into());
        }
        // 2) v4.3.8 改进:等待旧进程完全退出后,再自动启动新版本
        //    (避免新旧实例因单实例机制冲突:新版被拒/旧版被聚焦)
        let old_pid = std::process::id();
        spawn_wait_then_start(old_pid, &new_exe, &version);
    } else {
        // EBUSY 兜底：本进程退出释放锁后由 cmd 完成 备份→顶替→(超时兜底强杀旧进程)→启动新版
        let old_pid = std::process::id();
        let script = format!(
            "ping -n 5 127.0.0.1 >nul & copy /y \"{}\" \"{}\" >nul & move /y \"{}\" \"{}\" >nul & taskkill /F /PID {} >nul 2>&1 & ping -n 2 127.0.0.1 >nul & start \"\" \"{}\" --upgraded {}",
            exe.to_string_lossy().replace('"', ""),
            bak.to_string_lossy(),
            new_file.to_string_lossy(),
            new_exe.to_string_lossy(),
            old_pid,
            new_exe.to_string_lossy(),
            version
        );
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd.exe")
            .args(["/d", "/s", "/c", &script])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| format!("启动替换任务失败: {e}"))?;
    }
    let app_hold = APP_FOR_QUIT.lock().clone();
    if let Some(app) = app_hold {
        let a = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1200));
            a.exit(0);
        });
    }
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

/// v4.3.7：升级后自动添加/覆盖桌面快捷方式指向新版文件
/// （分离执行 PowerShell WScript.Shell，失败静默——快捷方式非关键路径）
fn refresh_desktop_shortcut(new_exe: &Path) {
    let exe = new_exe.to_string_lossy().replace('"', "");
    let script = format!(
        "$d=[Environment]::GetFolderPath('Desktop'); $ws=New-Object -ComObject WScript.Shell; $s=$ws.CreateShortcut(\"$d\\EasySSH.lnk\"); $s.TargetPath=\"{}\"; $s.WorkingDirectory=\"{}\"; $s.IconLocation=\"{},0\"; $s.Save()",
        exe,
        new_exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        exe
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
