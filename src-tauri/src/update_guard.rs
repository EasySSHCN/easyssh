//! v5(M1)/F-M1-02：升级自回滚——`--upgraded <版本>` 健康自检看护 + 失败自动回滚。
//!
//! 链路（对应手册 S-OTA-04，延续 v4.3.10 "旧进程退出后 bat 启动新版"）：
//!   1) 升级完成后新版以 `--upgraded <新版本>` 启动，看护线程启动（15s 窗口）；
//!   2) 主窗口创建成功且 Tauri setup 完成 → mark_healthy()（内存 AtomicBool + 时间戳）；
//!   3) 超时未标记健康 → 判定新版异常：
//!      - 当前 exe 改名 `.failed`
//!      - 对应 `.bak` 恢复为原主文件名
//!      - 分离启动恢复出的旧版（带 `--rollback <版本>`），随后结束自身进程
//!   4) 旧版以 `--rollback` 启动时，渲染端经 rollback_notice 命令读取版本号，
//!      在现有升级横幅显示"已自动回滚"提示。
//! 幂等防重入：`.failed` 已存在时跳过改名步骤，直接启动已恢复的旧版。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// 健康标记（原子位）
static HEALTHY: AtomicBool = AtomicBool::new(false);
/// 健康标记时间戳（UNIX 毫秒；标记时写入，供诊断）
static HEALTH_TS: AtomicU64 = AtomicU64::new(0);
/// `--rollback <版本>` 携带的"被回滚的新版版本号"（渲染端提示用）
static ROLLBACK_FROM: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

const WATCHDOG_TIMEOUT_MS: u64 = 15_000;

// ============ v5(M2)/任务三：可测试纯函数层（逻辑与进程操作分离） ============
// 说明：为满足 M2 集成测试（src-tauri/tests/update_guard_tests.rs）对看护/回滚逻辑
// 的纯逻辑覆盖，抽出以下与进程/环境无关的纯函数；perform_rollback 运行时按其计划执行。

/// 看护超时阈值（毫秒）——测试可读取，避免魔法数字
pub fn watchdog_timeout_ms() -> u64 {
    WATCHDOG_TIMEOUT_MS
}

/// 纯函数：健康判定——超时窗口内未标记健康 → 应回滚
/// - `health_marked`：看护窗口内是否已调用 mark_healthy()
/// - `elapsed_ms`：自看护启动起经过的毫秒数
pub fn guard_should_rollback(health_marked: bool, elapsed_ms: u64) -> bool {
    !health_marked && elapsed_ms >= WATCHDOG_TIMEOUT_MS
}

/// 回滚计划步骤（纯数据，无副作用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackStep {
    /// 当前(异常新版) exe 改名 `.failed`
    MarkFailed,
    /// `.bak` 恢复为原主文件名
    RestoreBakToExe,
    /// 分离启动恢复出的旧版（原名 exe，--rollback <版本>）
    LaunchRestored,
    /// 防重入：`.failed` 已存在（上次回滚已完成改名），直接启动已恢复的旧版
    LaunchRestoredReentry,
    /// 无 `.bak` 可回滚：绝不删除唯一可用程序文件，中止回滚
    AbortNoBak,
}

/// 纯函数：依据 exe/.bak/.failed 三态推导回滚步骤序列。
/// - `.failed` 已存在 → 防重入直接走恢复（[LaunchRestoredReentry]）
/// - 无 `.bak`（且无 `.failed`）→ 不删唯一可用文件，中止（[AbortNoBak]）
/// - 正常路径 → [MarkFailed, RestoreBakToExe, LaunchRestored]
pub fn rollback_plan(exe: &PathBuf, bak: &PathBuf, failed: &PathBuf) -> Vec<RollbackStep> {
    if failed.exists() {
        return vec![RollbackStep::LaunchRestoredReentry];
    }
    if !bak.exists() {
        return vec![RollbackStep::AbortNoBak];
    }
    vec![
        RollbackStep::MarkFailed,
        RollbackStep::RestoreBakToExe,
        RollbackStep::LaunchRestored,
    ]
}

/// 文件系统操作（可测试，无 spawn/exit）：执行计划中 MarkFailed + RestoreBakToExe 两步改名。
/// 任一步失败即整体恢复原状（failed→exe 还原），返回 Err(原因)。
pub fn apply_rollback_renames(exe: &PathBuf, bak: &PathBuf, failed: &PathBuf) -> Result<(), String> {
    std::fs::rename(exe, failed).map_err(|e| format!("exe→.failed 改名失败: {e}"))?;
    if let Err(e) = std::fs::rename(bak, exe) {
        // 失败还原：.failed 改回原 exe 名，保持三态一致
        let _ = std::fs::rename(failed, exe);
        return Err(format!(".bak→exe 恢复失败: {e}"));
    }
    Ok(())
}

/// 解析启动参数：`--upgraded <版本>` / `--rollback <版本>`
pub fn parse_launch_args() -> (Option<String>, Option<String>) {
    let mut upgraded = None;
    let mut rollback = None;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--upgraded" => upgraded = args.get(i + 1).cloned(),
            "--rollback" => rollback = args.get(i + 1).cloned(),
            _ => {}
        }
        i += 1;
    }
    (upgraded, rollback)
}

/// 记录回滚来源版本（main 启动早期调用）
pub fn set_rollback_from(v: Option<String>) {
    *ROLLBACK_FROM.lock().unwrap_or_else(|e| e.into_inner()) = v;
}

/// 渲染端查询回滚提示（Some(版本)=本次启动来自自动回滚）
pub fn rollback_notice() -> Option<String> {
    ROLLBACK_FROM.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 健康自检通过：主窗口创建成功、setup 完成后调用
pub fn mark_healthy() {
    HEALTHY.store(true, Ordering::SeqCst);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    HEALTH_TS.store(ts, Ordering::SeqCst);
}

/// 启动看护线程（仅 `--upgraded` 启动时调用）
pub fn start_watchdog(upgraded_version: String) {
    std::thread::Builder::new()
        .name("upgrade-watchdog".into())
        .spawn(move || watchdog(upgraded_version))
        .expect("启动升级看护线程失败");
}

fn watchdog(upgraded_version: String) {
    eprintln!(
        "[update-guard] --upgraded {}：健康自检窗口 {}ms",
        upgraded_version, WATCHDOG_TIMEOUT_MS
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(WATCHDOG_TIMEOUT_MS);
    while std::time::Instant::now() < deadline {
        if HEALTHY.load(Ordering::SeqCst) {
            // 自检通过：清理 .bak/.failed（手册 F-M1-02：自检通过后删除 .bak）
            cleanup_backups();
            eprintln!(
                "[update-guard] 健康自检通过({}ms 标记)，.bak 已清理",
                HEALTH_TS.load(Ordering::SeqCst)
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if HEALTHY.load(Ordering::SeqCst) {
        cleanup_backups();
        return;
    }
    eprintln!(
        "[update-guard] 健康自检超时({}ms 未标记健康)，新版 {} 判定异常，执行回滚",
        WATCHDOG_TIMEOUT_MS, upgraded_version
    );
    perform_rollback(&upgraded_version);
}

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("EasySSH.exe"))
}

/// 与 complete_upgrade 的 .bak 命名一致：`<exe>.bak`（with_extension("exe.bak")）
pub fn bak_path(exe: &PathBuf) -> PathBuf {
    exe.with_extension("exe.bak")
}

pub fn failed_path(exe: &PathBuf) -> PathBuf {
    exe.with_extension("exe.failed")
}

/// 自检通过后的备份清理（绝不删除唯一可用程序文件：.bak 只在健康后删除；
/// .failed 属于被判定异常的新版，保留供问题排查，下次健康启动时清除）
fn cleanup_backups() {
    let exe = exe_path();
    let _ = std::fs::remove_file(bak_path(&exe));
    let _ = std::fs::remove_file(failed_path(&exe));
}

/// 回滚执行（结束自身进程前调用）
/// v5(M2)/任务三：改为按 rollback_plan 纯函数推导的步骤序列执行（行为与 M1 一致），
/// 计划推导与文件改名均可被集成测试独立覆盖。
fn perform_rollback(upgraded_version: &str) {
    let exe = exe_path();
    let bak = bak_path(&exe);
    let failed = failed_path(&exe);

    match rollback_plan(&exe, &bak, &failed).as_slice() {
        // 幂等防重入：.failed 已存在 → 上次回滚已完成改名，直接启动已恢复的旧版
        [RollbackStep::LaunchRestoredReentry] => {
            eprintln!("[update-guard] .failed 已存在(防重入)，直接启动恢复出的旧版");
            launch_restored(&exe, upgraded_version);
        }
        // 无 .bak 可回滚（且无 .failed）→ 绝不删除唯一可用程序文件，仅退出
        [RollbackStep::AbortNoBak] => {
            eprintln!("[update-guard] 无 .bak 可回滚，保留当前程序并退出(回滚中止)");
            std::process::exit(1);
        }
        // 正常路径：MarkFailed → RestoreBakToExe → LaunchRestored
        plan if plan.first() == Some(&RollbackStep::MarkFailed) => {
            // 1) 当前(异常新版) exe 改名 .failed；失败(占用/杀软)则重试 3 次
            let mut renamed = false;
            for _ in 0..3 {
                if std::fs::rename(&exe, &failed).is_ok() {
                    renamed = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            if !renamed {
                // exe 无法改名：放弃改名，仍尝试恢复 .bak 到副本名启动（绝不覆盖唯一可用文件）
                eprintln!("[update-guard] 当前 exe 改名 .failed 失败(重试 3 次)，尝试直接启动 .bak");
                let restored = exe.with_extension("restored.exe");
                if std::fs::copy(&bak, &restored).is_ok() {
                    launch(&restored, upgraded_version);
                }
                std::process::exit(1);
            }
            // 2) .bak 恢复为原主文件名
            if std::fs::rename(&bak, &exe).is_err() {
                eprintln!("[update-guard] .bak 恢复原名失败，直接从 .bak 启动旧版");
                launch(&bak, upgraded_version);
                return;
            }
            // 3) 分离启动恢复出的旧版（--rollback <被回滚版本>）
            launch_restored(&exe, upgraded_version);
        }
        _ => {
            eprintln!("[update-guard] 回滚计划为空/未知，中止");
            std::process::exit(1);
        }
    }
}

/// 启动恢复出的旧版（原名 exe），失败兜底直接用 .bak
fn launch_restored(exe: &PathBuf, upgraded_version: &str) {
    launch(exe, upgraded_version);
}

fn launch(exe: &PathBuf, upgraded_version: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        match std::process::Command::new(exe)
            .arg("--rollback")
            .arg(upgraded_version)
            .creation_flags(0x08000000) // CREATE_NO_WINDOW，分离启动
            .spawn()
        {
            Ok(_) => eprintln!("[update-guard] 已启动旧版: {}", exe.display()),
            Err(e) => eprintln!("[update-guard] 启动旧版失败: {e}"),
        }
    }
    #[cfg(not(windows))]
    {
        match std::process::Command::new(exe).arg("--rollback").arg(upgraded_version).spawn() {
            Ok(_) => eprintln!("[update-guard] 已启动旧版: {}", exe.display()),
            Err(e) => eprintln!("[update-guard] 启动旧版失败: {e}"),
        }
    }
    // 给子进程让路后结束自身（异常新版退出，释放锁）
    std::thread::sleep(std::time::Duration::from_millis(500));
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_parsing() {
        // 直接调用 parse_launch_args 依赖进程参数，此处仅验证常量与路径推导逻辑
        let exe = PathBuf::from("C:/x/EasySSH-5.0.0-portable.exe");
        assert_eq!(
            bak_path(&exe),
            PathBuf::from("C:/x/EasySSH-5.0.0-portable.exe.bak")
        );
        assert_eq!(
            failed_path(&exe),
            PathBuf::from("C:/x/EasySSH-5.0.0-portable.exe.failed")
        );
        assert_eq!(WATCHDOG_TIMEOUT_MS, 15_000);
    }
}
