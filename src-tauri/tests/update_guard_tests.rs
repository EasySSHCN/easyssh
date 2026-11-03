//! v5(M2)/任务三：升级自回滚集成测试（F-M1-02 遗留闭环，手册要求 ≥5 条）
//!
//! 被测对象：src-tauri/src/update_guard.rs 的看护/回滚逻辑。
//! 该 crate 是 binary（无 lib.rs，且受文件归属约束不可新建 src/lib.rs），
//! 故以 `#[path]` 将 update_guard.rs 作为模块直接编入测试 crate——
//! 该模块仅依赖 std，可独立编译，不牵动 tauri/其他模块。
//!
//! CI 策略：纯逻辑测试（含仅文件系统重命名的测试）默认执行；
//! 涉及真实进程 spawn/exit 的验证用 `#[ignore]` 标注，手动运行：
//!   cargo test -p easyssh --test update_guard_tests -- --ignored
//!
//! 手册对应：S-OTA-04 / F-M1-02（升级自回滚）、F-M2-07（测试地基）

#[path = "../src/update_guard.rs"]
mod update_guard;

use std::fs;
use std::path::PathBuf;

use update_guard::{apply_rollback_renames, bak_path, failed_path, guard_should_rollback,
                   rollback_plan, watchdog_timeout_ms, RollbackStep};

/// 构造隔离临时目录（测试结束自动清理）
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "easyssh-guard-test-{}-{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&base).expect("创建临时目录失败");
        TempDir(base)
    }
    fn path(&self) -> PathBuf {
        self.0.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 便捷构造：exe/bak/failed 三路径（文件名与运行时推导规则一致）
fn triple(dir: &PathBuf, name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let exe = dir.join(name);
    (exe.clone(), bak_path(&exe), failed_path(&exe))
}

// ---------- 纯逻辑测试（CI 默认执行） ----------

/// 用例 1：健康标记及时 → 不回滚
#[test]
fn t01_healthy_in_time_means_no_rollback() {
    let timeout = watchdog_timeout_ms();
    assert_eq!(timeout, 15_000, "看护窗口应保持 M1 规格的 15s");
    // 窗口内任意时刻已标记健康 → 不回滚
    assert!(!guard_should_rollback(true, 0));
    assert!(!guard_should_rollback(true, timeout - 1));
    assert!(!guard_should_rollback(true, timeout));
    assert!(!guard_should_rollback(true, timeout * 10));
}

/// 用例 2：超时未标记 → 回滚判定成立，且回滚计划正确
#[test]
fn t02_timeout_without_health_mark_triggers_rollback_plan() {
    let timeout = watchdog_timeout_ms();
    // 未标记健康 + 已超时 → 回滚
    assert!(guard_should_rollback(false, timeout));
    assert!(guard_should_rollback(false, timeout + 1));
    // 未标记健康但窗口未到 → 尚不回滚（看护线程继续轮询）
    assert!(!guard_should_rollback(false, timeout - 1));

    // 回滚计划正确性：正常三态（exe 在、.bak 在、无 .failed）
    let td = TempDir::new("plan-normal");
    let (exe, bak, failed) = triple(&td.path(), "EasySSH-5.0.0-portable.exe");
    fs::write(&exe, b"new").unwrap();
    fs::write(&bak, b"old").unwrap();
    assert_eq!(
        rollback_plan(&exe, &bak, &failed),
        vec![
            RollbackStep::MarkFailed,
            RollbackStep::RestoreBakToExe,
            RollbackStep::LaunchRestored
        ]
    );
}

/// 用例 3：无 .bak → 不删唯一可用文件（计划为 AbortNoBak，exe 原样保留）
#[test]
fn t03_no_bak_never_deletes_only_executable() {
    let td = TempDir::new("no-bak");
    let (exe, bak, failed) = triple(&td.path(), "EasySSH-5.0.0-portable.exe");
    fs::write(&exe, b"only-executable").unwrap();
    assert!(!bak.exists(), "前置：.bak 不存在");

    assert_eq!(rollback_plan(&exe, &bak, &failed), vec![RollbackStep::AbortNoBak]);
    // 计划为中止：唯一可用程序文件必须原样存在（未被改名/删除）
    assert!(exe.exists(), "无 .bak 时绝不能动唯一可用 exe");
    assert_eq!(fs::read(&exe).unwrap(), b"only-executable");
    assert!(!failed.exists());
}

/// 用例 4：.failed 已存在 → 防重入直接走恢复（不重复改名）
#[test]
fn t04_failed_marker_reentry_goes_straight_to_restore() {
    let td = TempDir::new("reentry");
    let (exe, bak, failed) = triple(&td.path(), "EasySSH-5.0.0-portable.exe");
    fs::write(&bak, b"old").unwrap();
    fs::write(&failed, b"broken-new").unwrap();

    assert_eq!(
        rollback_plan(&exe, &bak, &failed),
        vec![RollbackStep::LaunchRestoredReentry]
    );
    // 防重入路径不得再次触碰 exe/.bak
    assert!(!exe.exists());
    assert!(bak.exists(), "防重入时 .bak 应保持已恢复就绪状态");
}

/// 用例 5：回滚改名执行后 exe/.bak/.failed 状态断言（仅文件系统，无进程操作）
#[test]
fn t05_rollback_renames_final_state() {
    let td = TempDir::new("apply");
    let (exe, bak, failed) = triple(&td.path(), "EasySSH-5.0.0-portable.exe");
    fs::write(&exe, b"broken-new-version").unwrap();
    fs::write(&bak, b"good-old-version").unwrap();

    // 执行 MarkFailed + RestoreBakToExe 两步
    apply_rollback_renames(&exe, &bak, &failed).expect("回滚改名应成功");

    // 断言终态：
    assert_eq!(fs::read(&exe).unwrap(), b"good-old-version", "原主文件名应恢复为旧版内容");
    assert_eq!(fs::read(&failed).unwrap(), b"broken-new-version", "异常新版应保留为 .failed 供排查");
    assert!(!bak.exists(), ".bak 已恢复原名，不应残留");
    assert_eq!(
        rollback_plan(&exe, &bak, &failed),
        vec![RollbackStep::LaunchRestoredReentry],
        "终态应与防重入一致（再次触发时直接启动已恢复旧版）"
    );
}

/// 用例 5b：改名失败时恢复原状（例如 .failed 路径非法 → exe 保持原样）
#[test]
fn t05b_rollback_rename_failure_restores_state() {
    let td = TempDir::new("apply-fail");
    let (exe, bak, _failed) = triple(&td.path(), "EasySSH-5.0.0-portable.exe");
    fs::write(&exe, b"broken-new").unwrap();
    fs::write(&bak, b"old").unwrap();
    // 制造 .failed 为目录 → rename exe→failed 必失败
    fs::create_dir_all(&failed_path(&exe)).unwrap();

    assert!(apply_rollback_renames(&exe, &bak, &failed_path(&exe)).is_err());
    assert_eq!(fs::read(&exe).unwrap(), b"broken-new", "失败时 exe 必须原样保留");
}

/// 用例：路径推导规则回归（与 M1 complete_upgrade 命名一致）
#[test]
fn t06_path_derivation_matches_m1_naming() {
    let exe = PathBuf::from("C:/x/EasySSH-5.0.0-portable.exe");
    assert_eq!(bak_path(&exe), PathBuf::from("C:/x/EasySSH-5.0.0-portable.exe.bak"));
    assert_eq!(failed_path(&exe), PathBuf::from("C:/x/EasySSH-5.0.0-portable.exe.failed"));
}

// ---------- 进程操作类（#[ignore] 手动验证，CI 不跑） ----------

/// 手动验证：真实 spawn 子进程并等待退出（验证本测试环境的进程操作可行性）。
/// 运行：cargo test -p easyssh --test update_guard_tests -- --ignored
/// 说明：update_guard 的 launch()/process::exit() 会终止测试进程自身，
/// 无法在 cargo test 内直接调用；完整链路（--upgraded 超时→真实回滚→--rollback 启动）
/// 依赖双版本 exe 与 GUI 环境，属端到端人工验收，故此处仅验证 spawn/exit 原语，
/// 完整升级自回滚链路按手册 S-OTA-04 以"真实发版演练"方式人工验证（见 docs/v5/集成测试.md）。
#[test]
#[ignore = "真实进程操作：手动运行 cargo test -- --ignored 验证"]
fn t90_manual_real_process_spawn_and_exit() {
    let out = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
        .args(if cfg!(windows) {
            vec!["/d", "/c", "exit /b 0"]
        } else {
            vec!["-c", "exit 0"]
        })
        .status()
        .expect("spawn 失败");
    assert!(out.success());
}
