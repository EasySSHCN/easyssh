/* v5.1.3/F-WV2 — WebView2 Runtime 缺失容错引导
 *
 * 背景：EasySSH(Tauri) 依赖系统 WebView2 Evergreen Runtime；Windows 10/11 自带，
 * 但 Windows Server 2016/2019 等不自带，启动即 panic 闪退（crash-*.log：
 * "Could not find the webview runtime"）。
 *
 * 本模块在 Tauri Builder.run()（即创建 WebView）之前运行：
 *   1) 检测 WebView2 Runtime（微软官方注册表检测点，三处）；
 *   2) 缺失 → 弹原生 MessageBox 请求授权；
 *      - 同意：从微软官方 fwlink 下载 Evergreen Bootstrapper（约 2MB，
 *        安装器自己再从微软 CDN 拉取运行时），runas 提权静默安装，等待完成后复检；
 *      - 拒绝：弹确认框"……本软件无法运行 将自动退出。确认要退出吗？"
 *        点"是"退出进程；点"否"回到授权下载安装流程（循环）。
 *   3) 下载/安装失败 → 弹错误框后回到授权流程，用户可重试或选择退出。
 *
 * 无新增重量级依赖：对话框/ShellExecuteEx 直接声明 Win32 FFI；
 * 注册表用 winreg；下载复用已有 ureq。全程不涉及任何会话/密码数据。
 */

#[cfg(windows)]
pub fn ensure_runtime() {
    // 已装好（含 Evergreen/固定版本/嵌入式），直接放行
    if detect_version().is_some() {
        return;
    }
    loop {
        // ---- 步骤 1：授权提示（含手工安装指引） ----
        let consent = msgbox(
            "EasySSH",
            "未检测到 Microsoft Edge WebView2 Runtime，\n\
             本软件界面依赖该微软官方组件（Windows Server 系统通常不自带）。\n\n\
             【方式一：自动安装（推荐）】\n\
             点\"是\"后，本软件将自动下载并静默安装\n\
             微软官方 WebView2 Evergreen Runtime（约 2MB 官方引导器，\n\
             运行时会从微软官方 CDN 下载；安装需在弹出的管理员授权中点\"是\"）。\n\n\
             【方式二：手工安装】\n\
             点\"否\"并在下一窗口选择退出后，可在任意能上网的电脑打开\n\
             https://developer.microsoft.com/microsoft-edge/webview2 \n\
             下载 \"Evergreen Standalone Installer\" 的 x64 版本\n\
             （MicrosoftEdgeWebView2RuntimeInstallerX64.exe），\n\
             拷贝到本机双击安装，完成后重新运行 EasySSH。\n\n\
             是否同意自动下载并安装微软官方 WebView2 Evergreen Runtime？",
            MB_ICONWARNING | MB_YESNO | MB_TOPMOST,
        );
        if consent != IDYES {
            // ---- 步骤 4：拒绝 → 退出确认（点"否"回到授权流程） ----
            let quit = msgbox(
                "EasySSH",
                "由于拒绝安装微软官方的 WebView2 Evergreen Runtime\n\
                 本软件无法运行 将自动退出。\n\n\
                 确认要退出吗？",
                MB_ICONWARNING | MB_YESNO | MB_TOPMOST,
            );
            if quit == IDYES {
                std::process::exit(1);
            }
            continue; // 点"否" → 转回授权下载安装
        }

        // ---- 步骤 2：官方引导器下载 ----
        match download_bootstrapper() {
            Ok(installer_path) => {
                // ---- 步骤 3：提权静默安装 + 等待 + 复检 ----
                match install_and_wait(&installer_path) {
                    Ok(Some(ver)) => {
                        msgbox(
                            "EasySSH",
                            &format!(
                                "WebView2 Runtime {} 安装完成，EasySSH 即将启动。",
                                ver
                            ),
                            MB_ICONINFORMATION | MB_TOPMOST,
                        );
                        return; // 继续正常启动
                    }
                    _ => {}
                }
                // 安装执行了但复检仍失败（或启动失败）
                msgbox(
                    "EasySSH",
                    "WebView2 Runtime 安装未成功（可能取消了管理员权限确认，\n\
                     或安装被安全软件拦截）。\n\n请重试或手动安装后重新运行 EasySSH。",
                    MB_ICONERROR | MB_TOPMOST,
                );
            }
            Err(e) => {
                msgbox(
                    "EasySSH",
                    &format!("下载微软官方安装器失败：{}\n\n请检查网络后重试。", e),
                    MB_ICONERROR | MB_TOPMOST,
                );
            }
        }
        // 失败路径都回到 loop 顶部的授权提示
        if detect_version().is_some() {
            return;
        }
    }
}

#[cfg(not(windows))]
pub fn ensure_runtime() {}

/* ---------- 检测 ---------- */

/// WebView2 Runtime 的微软官方检测点（Evergreen 与固定版本都写此处）：
/// 值 pv = 版本号字符串。
#[cfg(windows)]
const CLIENT_ID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

#[cfg(windows)]
fn detect_version() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;
    let keys = [
        // HKLM 64 位视图（64 位系统 machine 级安装的标准位置）
        (r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients", HKEY_LOCAL_MACHINE),
        // HKLM 32 位视图
        (r"SOFTWARE\Microsoft\EdgeUpdate\Clients", HKEY_LOCAL_MACHINE),
        // HKCU（per-user 安装）
        (r"Software\Microsoft\EdgeUpdate\Clients", HKEY_CURRENT_USER),
    ];
    for (path, hive) in keys {
        if let Ok(clients) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) {
            if let Ok(sub) = clients.open_subkey_with_flags(CLIENT_ID, KEY_READ) {
                if let Ok(pv) = sub.get_value::<String, _>("pv") {
                    if !pv.trim().is_empty() && pv.trim() != "0.0.0.0" {
                        return Some(pv.trim().to_string());
                    }
                }
            }
        }
    }
    None
}

/* ---------- 下载 ---------- */

/// 微软官方 Evergreen Bootstrapper 固定分发链接（developer.microsoft.com/webview2 页面
/// 的 "Evergreen Bootstrapper" 按钮即指向此 fwlink）。
#[cfg(windows)]
const BOOTSTRAPPER_URL: &str = "https://go.microsoft.com/fwlink/?linkid=2124703";

#[cfg(windows)]
fn download_bootstrapper() -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir();
    let path = dir.join("MicrosoftEdgeWebview2Setup.exe");
    let resp = ureq::get(BOOTSTRAPPER_URL)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|e| format!("{e}"))?;
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(&path).map_err(|e| format!("{e}"))?;
    std::io::copy(&mut reader, &mut f).map_err(|e| format!("{e}"))?;
    Ok(path)
}

/* ---------- 安装（runas 提权 + 等待 + 复检） ---------- */

#[cfg(windows)]
fn install_and_wait(installer: &std::path::Path) -> Result<Option<String>, String> {
    use std::os::windows::ffi::OsStrExt;
    let exe: Vec<u16> = std::ffi::OsStr::new(installer)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = std::ffi::OsStr::new("/silent /install")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb: Vec<u16> = std::ffi::OsStr::new("runas")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut sei = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: std::ptr::null_mut(),
        lpVerb: verb.as_ptr(),
        lpFile: exe.as_ptr(),
        lpParameters: params.as_ptr(),
        lpDirectory: std::ptr::null(),
        nShow: SW_SHOWNORMAL,
        hInstApp: 0,
        lpIDList: std::ptr::null_mut(),
        lpClass: std::ptr::null(),
        hkeyClass: std::ptr::null_mut(),
        dwHotKey: 0,
        hIcon: std::ptr::null_mut(),
    };
    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 {
        // 31=SE_ERR_NOASSOC 类错误；用户在 UAC 点"取消"也会走这里
        return Err(format!("无法启动安装器（错误码 {}，可能取消了管理员授权）", sei.hInstApp));
    }
    // 等待安装进程退出（SEE_MASK_NOCLOSEPROCESS → hInstApp 即进程句柄）
    if sei.hInstApp != 0 {
        unsafe { WaitForSingleObject(sei.hInstApp as _, 600_000) };
        unsafe { CloseHandle(sei.hInstApp as _) };
    }
    // 安装器可能派生提权子进程，句柄退出后注册表未必立即可见 → 轮询复检
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        if let Some(v) = detect_version() {
            return Ok(Some(v));
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    Ok(None)
}

/* ---------- 原生 Win32 FFI（不引 windows crate，保持包体） ---------- */

#[cfg(windows)]
const MB_ICONWARNING: u32 = 0x30;
#[cfg(windows)]
const MB_ICONERROR: u32 = 0x10;
#[cfg(windows)]
const MB_ICONINFORMATION: u32 = 0x40;
#[cfg(windows)]
const MB_YESNO: u32 = 0x4;
#[cfg(windows)]
const MB_TOPMOST: u32 = 0x40000;
#[cfg(windows)]
const MB_SETFOREGROUND: u32 = 0x10000;
#[cfg(windows)]
const IDYES: i32 = 6;

#[cfg(windows)]
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;
#[cfg(windows)]
const SW_SHOWNORMAL: i32 = 1;

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)] // 与 Win32 SHELLEXECUTEINFW 官方字段名保持一致
struct SHELLEXECUTEINFOW {
    cbSize: u32,
    fMask: u32,
    hwnd: *mut core::ffi::c_void,
    lpVerb: *const u16,
    lpFile: *const u16,
    lpParameters: *const u16,
    lpDirectory: *const u16,
    nShow: i32,
    hInstApp: isize,
    lpIDList: *mut core::ffi::c_void,
    lpClass: *const u16,
    hkeyClass: *mut core::ffi::c_void,
    dwHotKey: u32,
    hIcon: *mut core::ffi::c_void,
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
}

#[cfg(windows)]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteExW(info: *mut SHELLEXECUTEINFOW) -> i32;
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn WaitForSingleObject(handle: *mut core::ffi::c_void, ms: u32) -> u32;
    fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
}

#[cfg(windows)]
fn msgbox(caption: &str, text: &str, flags: u32) -> i32 {
    use std::os::windows::ffi::OsStrExt;
    let enc = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    };
    let t = enc(text);
    let c = enc(caption);
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), flags | MB_SETFOREGROUND) }
}
