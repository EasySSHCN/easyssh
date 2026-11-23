//! Windows 服务形态（方案第 5 节：`sc create` 双形态之一）。
//!
//! 直接以 `windows-sys` 手写 SCM 调度（首版独立实现，不引入额外 crate）：
//! - `easyssh-agent --service`：在 SCM 内运行（StartServiceCtrlDispatcher）；
//! - 服务名 `EasySSHAgent`，安装/卸载由 README 固化：
//!   `sc create EasySSHAgent binPath= "C:\...\easyssh-agent.exe --service" start= auto`
//!   / `sc delete EasySSHAgent`；
//! - 状态机：START_PENDING → RUNNING →（STOP_PENDING → STOPPED）。

#![cfg(windows)]

use std::sync::OnceLock;
use tokio::sync::watch;

use crate::{agent, config};

/// 服务名（`sc create EasySSHAgent ...`）。
pub const SERVICE_NAME: &str = "EasySSHAgent";

/// 全局停机信号：SCM STOP 控制码 → agent 主循环退出。
static SHUTDOWN_TX: OnceLock<watch::Sender<bool>> = OnceLock::new();

fn shutdown_tx() -> &'static watch::Sender<bool> {
    SHUTDOWN_TX.get_or_init(|| watch::channel(false).0)
}

pub fn shutdown_rx() -> watch::Receiver<bool> {
    shutdown_tx().subscribe()
}

// ---------------------------------------------------------------------
// windows-sys 原始类型别名（API 表面）
// ---------------------------------------------------------------------

use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW,
};

const CONTROLS_ACCEPTED: u32 = SERVICE_ACCEPT_STOP;

extern "system" fn service_main(argc: u32, argv: *mut *mut u16) {
    let _ = (argc, argv); // SCM 不传有含义参数；配置读可执行文件同目录 agent.toml
    let name_w = wide(&SERVICE_NAME);
    let handle =
        unsafe { RegisterServiceCtrlHandlerW(name_w.as_ptr(), Some(service_ctrl_handler)) };
    if handle == 0 {
        return;
    }
    report_status(handle, SERVICE_START_PENDING, 0);

    // 真正的业务跑在 tokio 运行时里；SCM 主线程只负责状态回报。
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let outcome = (|| -> Result<(), String> {
        let cfg = match config::AgentConfig::load_from_path(&config::default_config_path()) {
            Ok(c) => c,
            Err(e) => return Err(format!("agent.toml 加载失败: {e}")),
        };
        let token = crate::store::load_token(cfg.token_path.as_deref())
            .map_err(|e| format!("token 读取失败: {e}"))?
            .ok_or_else(|| "尚未 enroll：先运行 easyssh-agent --enroll <TOKEN>".to_string())?;
        rt.block_on(agent::run_agent(cfg, token, shutdown_rx()));
        Ok(())
    })();

    report_status(handle, SERVICE_STOP_PENDING, 0);
    let exit_code = match outcome {
        Ok(()) => 0,
        Err(msg) => {
            eprintln!("easyssh-agent 服务退出: {msg}");
            1067 // ERROR_PROCESS_ABORTED 语义：进程异常终止
        }
    };
    report_status(handle, SERVICE_STOPPED, exit_code);
}

extern "system" fn service_ctrl_handler(control: u32) {
    if control == SERVICE_CONTROL_STOP {
        // 通知 agent 主循环退出；STOPPED 由 service_main 收尾时回报。
        let _ = shutdown_tx().send(true);
    }
}

fn report_status(handle: SERVICE_STATUS_HANDLE, state: u32, exit_code: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: 0x10, // SERVICE_WIN32_OWN_PROCESS
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            CONTROLS_ACCEPTED
        } else {
            0
        },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: 0,
    };
    unsafe {
        SetServiceStatus(handle, &status);
    }
}

/// 在 SCM 内运行（`--service` 入口）。不在 SCM 上下文中调用会立即失败返回。
pub fn run_as_windows_service() -> bool {
    let mut service_name_w = wide(&SERVICE_NAME);
    let mut table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name_w.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: std::ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    let ok = unsafe { StartServiceCtrlDispatcherW(table.as_mut_ptr()) };
    if ok == 0 {
        eprintln!("easyssh-agent --service 必须由服务控制管理器启动（sc start EasySSHAgent）");
    }
    ok != 0
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
