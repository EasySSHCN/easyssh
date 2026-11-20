/* EasySSH v5.2.2/R-20261119-005:portable 免安装版任务栏固定引导(Rust 侧)
 *
 * 职责:
 *   1) taskbar_pin_status —— 扫描任务栏固定区(%APPDATA%\Microsoft\Internet Explorer\
 *      Quick Launch\User Pinned\TaskBar)下全部 .lnk,按字节序列(ASCII/UTF-16LE)匹配
 *      当前 exe 文件名,判定是否已固定。任何错误一律静默降级为 pinned=false。
 *   2) taskbar_pin_do —— 确保开始菜单 Programs 目录下存在 EasySSH.lnk(缺则经
 *      WScript.Shell COM 创建),再用 Shell.Application COM 的 InvokeVerb/DoIt 走
 *      系统「固定到任务栏」正规流程;完成后复检,仍未固定 → need_manual=true
 *      (Win11 动词受限场景,由前端引导用户右键手动固定)。
 *   3) set_app_user_model_id —— 启动早期设置 AUMID "EasySSH.Client",
 *      保证任务栏图标分组/固定标识一致(ole32.SetCurrentProcessExplicitAppUserModelID)。
 *
 * 硬性约束:所有 std::process::Command 调 powershell.exe 必须
 *   -NoProfile -NonInteractive -WindowStyle Hidden 且 creation_flags(0x08000000)
 *   (CREATE_NO_WINDOW,CommandExt 为 std 自带,零新增依赖),
 *   从源头消除 GUI 进程下 PowerShell 黑窗闪现。 */

/* ---------- Windows 实现 ---------- */
#[cfg(windows)]
pub mod imp {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const LNK_NAME: &str = "EasySSH.lnk";

    /// 隐藏窗口执行 PowerShell 脚本,返回 stdout(失败返回 None,静默)
    fn run_ps(script: &str) -> Option<String> {
        let out = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// 路径安全内插到 PS 单引号字符串(单引号翻倍转义)
    fn psq(s: &str) -> String {
        s.replace('\'', "''")
    }

    /// 当前 exe 完整路径(字符串形式,便于前端展示)
    fn exe_path_string() -> String {
        std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    fn pinned_taskbar_dir() -> Result<PathBuf, String> {
        let appdata = std::env::var("APPDATA").map_err(|_| "无法获取 APPDATA".to_string())?;
        Ok(PathBuf::from(appdata)
            .join("Microsoft")
            .join("Internet Explorer")
            .join("Quick Launch")
            .join("User Pinned")
            .join("TaskBar"))
    }

    fn programs_dir() -> Result<PathBuf, String> {
        let appdata = std::env::var("APPDATA").map_err(|_| "无法获取 APPDATA".to_string())?;
        Ok(PathBuf::from(appdata).join("Microsoft").join("Windows").join("Start Menu").join("Programs"))
    }

    /// 在字节流中搜索模式(ASCII / UTF-16LE 两种编码都查)
    fn bytes_contain(hay: &[u8], needle: &[u8]) -> bool {
        if needle.is_empty() || hay.len() < needle.len() {
            return false;
        }
        hay.windows(needle.len()).any(|w| w == needle)
    }

    /// 复检:当前 exe 是否已固定(错误 → false,静默)
    pub fn status() -> (bool, String) {
        let exe = exe_path_string();
        let pinned = status_inner().unwrap_or(false);
        (pinned, exe)
    }

    fn status_inner() -> Result<bool, String> {
        let exe = std::env::current_exe().map_err(|_| "无法获取当前 exe".to_string())?;
        let name = exe
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or("无法获取 exe 文件名")?;
        let dir = pinned_taskbar_dir()?;
        if !dir.is_dir() {
            return Ok(false); // 目录不存在 = 必然未固定
        }
        // 两种编码候选字节序列
        let ascii_pat = name.as_bytes().to_vec();
        let u16_pat: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        for e in std::fs::read_dir(&dir).map_err(|e| format!("读取固定区失败: {e}"))?.flatten() {
            let p = e.path();
            if p.extension().map(|x| x.eq_ignore_ascii_case("lnk")).unwrap_or(false) {
                if let Ok(bytes) = std::fs::read(&p) {
                    if bytes_contain(&bytes, &ascii_pat) || bytes_contain(&bytes, &u16_pat) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// 确保开始菜单 Programs\EasySSH.lnk 存在(缺则经 WScript.Shell COM 创建)
    fn ensure_programs_shortcut() -> Result<PathBuf, String> {
        let programs = programs_dir()?;
        std::fs::create_dir_all(&programs).map_err(|e| format!("创建 Programs 目录失败: {e}"))?;
        let lnk = programs.join(LNK_NAME);
        if !lnk.exists() {
            let exe = std::env::current_exe().map_err(|_| "无法获取当前 exe".to_string())?;
            let workdir = exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let script = format!(
                "$ws = New-Object -ComObject WScript.Shell; \
                 $s = $ws.CreateShortcut('{}'); \
                 $s.TargetPath = '{}'; \
                 $s.WorkingDirectory = '{}'; \
                 $s.Save()",
                psq(&lnk.to_string_lossy()),
                psq(&exe.to_string_lossy()),
                psq(&workdir),
            );
            if run_ps(&script).is_none() {
                return Err("创建开始菜单快捷方式失败".to_string());
            }
        }
        Ok(programs)
    }

    /// 经 Shell.Application COM 触发「固定到任务栏」动词(兼容多语言动词名)
    fn invoke_pin_verb(programs: &PathBuf) -> Result<(), String> {
        // 动词名因系统语言而异(英文 pintotaskbar / 中文 固定到任务栏 / 部分「钉选」),
        // 用 Verbs() 集合正则匹配后 DoIt(),比硬编码 InvokeVerb('pintotaskbar') 更稳。
        let script = format!(
            "$sh = New-Object -ComObject Shell.Application; \
             $d = $sh.NameSpace('{}'); \
             if ($null -eq $d) {{ exit 2 }} \
             $it = $d.ParseName('{}'); \
             if ($null -eq $it) {{ exit 3 }} \
             $v = $it.Verbs() | Where-Object {{ $_.Name -match '(?i)pintotaskbar|pin to taskbar|固定到任务栏|钉选' }} | Select-Object -First 1; \
             if ($null -eq $v) {{ exit 4 }} \
             $v.DoIt(); \
             Write-Output 'PIN_OK'; \
             exit 0",
            psq(&programs.to_string_lossy()),
            LNK_NAME,
        );
        match run_ps(&script) {
            None => Err("调用 Shell.Application 固定流程失败".to_string()),
            Some(s) => {
                // 脚本成功时回显 PIN_OK 哨兵;缺失说明 NameSpace/ParseName/动词查找任一步失败
                if s.contains("PIN_OK") {
                    Ok(())
                } else {
                    Err("未找到可用的固定动词(可能系统限制)".to_string())
                }
            }
        }
    }

    /// 一键固定:建快捷方式 → 触发动词 → 复检。返回 (ok, need_manual, message)
    pub fn do_pin() -> (bool, bool, String) {
        match do_pin_inner() {
            Ok((pinned, msg)) => (true, !pinned, msg),
            Err(e) => (false, true, e), // 失败也要引导手动固定
        }
    }

    fn do_pin_inner() -> Result<(bool, String), String> {
        let programs = ensure_programs_shortcut()?;
        invoke_pin_verb(&programs)?;
        // 固定动作落盘有延迟,稍候再复检
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let (pinned, _) = status();
        let msg = if pinned {
            String::new()
        } else {
            format!(
                "未能自动固定(系统限制)。可在开始菜单中右键点击 EasySSH 选择「固定到任务栏」。快捷方式位置: {}",
                programs.join(LNK_NAME).to_string_lossy()
            )
        };
        Ok((pinned, msg))
    }

    /// 启动早期设置 AppUserModelID(任务栏图标分组/固定一致性)
    pub fn set_app_user_model_id() {
        #[link(name = "ole32")]
        extern "system" {
            fn SetCurrentProcessExplicitAppUserModelID(AppID: *const u16) -> i32;
        }
        // 宽字符串必须以 null 结尾
        let mut w: Vec<u16> = "EasySSH.Client".encode_utf16().collect();
        w.push(0);
        unsafe {
            // 返回 HRESULT 失败仅静默(不影响启动;失败场景罕见)
            let _ = SetCurrentProcessExplicitAppUserModelID(w.as_ptr());
        }
    }
}

/* ---------- 非 Windows 桩(保证跨平台可编译;实际目标平台为 Windows) ---------- */
#[cfg(not(windows))]
pub mod imp {
    pub fn status() -> (bool, String) {
        (true, String::new()) // 非 Windows 视为已处理,前端不再弹窗
    }
    pub fn do_pin() -> (bool, bool, String) {
        (false, false, "仅 Windows 支持".to_string())
    }
    pub fn set_app_user_model_id() {}
}

/* ---------- Tauri 命令(Windows only;非 Windows 由 main.rs 桩兜底) ---------- */

/// v5.2.2/R-20261119-005:查询当前 exe 是否已固定到任务栏
#[cfg(windows)]
#[tauri::command]
pub fn taskbar_pin_status() -> serde_json::Value {
    let (pinned, exe_path) = imp::status();
    serde_json::json!({ "pinned": pinned, "exePath": exe_path })
}

/// v5.2.2/R-20261119-005:一键固定(自动建快捷方式 + 系统正规固定流程)
#[cfg(windows)]
#[tauri::command]
pub fn taskbar_pin_do() -> serde_json::Value {
    let (ok, need_manual, message) = imp::do_pin();
    serde_json::json!({ "ok": ok, "needManual": need_manual, "message": message })
}

/* 非 Windows 平台的 Tauri 命令桩(main.rs 注册用;返回恒不弹窗/无需手动) */
#[cfg(not(windows))]
#[tauri::command]
pub fn taskbar_pin_status() -> serde_json::Value {
    serde_json::json!({ "pinned": true, "exePath": "" })
}

#[cfg(not(windows))]
#[tauri::command]
pub fn taskbar_pin_do() -> serde_json::Value {
    serde_json::json!({ "ok": true, "needManual": false, "message": "" })
}
