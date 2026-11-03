/* EasySSH v4：SFTP 传输队列（对应 3.x xfer:* 命令）
 * v5(M2)/F-M2-01 传输管道化：大文件读写按范围切分为 N 个并行 worker
 *   （默认 8，可配 1..=16），每个 worker 以 1MiB 块顺序推进自己的区间，
 *   在途请求 = worker 数（上传时 russh-sftp File 内部还有写确认流水线），
 *   千兆局域网/高延迟链路吞吐相比单请求串行可提升数倍。
 * v5(M2)/F-M2-02 断点续传：任务失败/取消时按 worker 记录已传偏移
 *   （内存 tasks 表 + 磁盘 xfer-resume.json 双份），重传时带 resume=true
 *   即从断点继续；续传前校验远端/本地已有部分大小。
 * 进度仍经 "xfer:progress" 事件推送（协议向后兼容：原 id/status/received/total
 * 字段不变，另附队列模态框所需的 taskId/phase/bytesDone/bytesTotal 等字段）。
 * v5(M4)/F-M4-02 队列增强：
 *   - 暂停/恢复：worker 每传完一个块检查暂停旗标，暂停时停止发起新请求（在途块完成），
 *     状态 paused；恢复后从当前断点继续（断点天然保留，不写 journal 也无需恢复 journal）。
 *     与取消语义区分：取消=终止任务（断点写 journal，可▶续传重建任务）；暂停=任务保活、断点在内存。
 *   - 优先级调度：任务带 priority(high/normal/low)，由中央调度器按优先级分配全局并发
 *     配额（同优先级按入队顺序），高优先任务先拿到 worker 槽位。
 *   - 进度事件新增 priority 字段（前端 UI 展示用）。
 * v5(M4.1)/F-M4-02 单任务限速：xfer_start 增加可选 speedLimit（字节/秒，0/缺省=不限），
 *   总限速按 worker 数均分为每 worker 速率，worker 每传一个块后块间 sleep 推导延迟
 *   （chunk_delay_ms）；节流发生在暂停/取消门控之后，暂停=停等、恢复=按限速继续，
 *   语义完全兼容；进度事件协议不变，旧调用（不带 speedLimit）零变更。 */
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use russh_sftp::protocol::OpenFlags;
use serde_json::Value;

use parking_lot::Mutex;
use russh_sftp::client::SftpSession;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// v5(M2)：单 worker 每次读/写的块大小（1MiB）
pub const CHUNK: u64 = 1024 * 1024;
/// v5(M2)：默认并行 worker 数（在途请求数）；可通过 start 的 pipeline 参数覆盖
pub const DEFAULT_PIPELINE: usize = 8;
pub const MAX_PIPELINE: usize = 16;
/// 续传状态持久化文件（随 exe 目录，便携语义）
fn resume_journal_path() -> PathBuf {
    crate::configstate::default_base_dir().join("xfer-resume.json")
}

#[derive(Clone)]
pub struct XferState {
    pub sftps: crate::sftp::SftpMap,
    pub tasks: ArcTasks,
    pub cancels: ArcCancels,
    /// v5(M4)：任务 id → 暂停旗标（true=已请求暂停，worker 传完在途块后停等）
    pub pauses: ArcPauses,
    /// v5(M4)：任务 id → 启动信号（调度器分配到并发槽位后 send(true)）
    pub starts: ArcStarts,
    /// v5(M4)：调度器事件通道（入队/完成），调度器任务首次 start 时惰性拉起
    pub disp: ArcDisp,
    pub disp_started: std::sync::Arc<AtomicBool>,
    /// v5(M4)：全局并发上限（替代旧 Semaphore 闸门，由调度器仲裁）
    pub capacity: usize,
    /// v5(M2)：任务 id → 各 worker 已传偏移（相对区间起点）；失败/取消时折入任务记录
    pub range_done: ArcRangeDone,
}

#[derive(Clone, Default)]
pub struct ArcTasks(pub std::sync::Arc<Mutex<HashMap<u64, Value>>>);
#[derive(Clone, Default)]
pub struct ArcCancels(pub std::sync::Arc<Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>>);
#[derive(Clone, Default)]
pub struct ArcRangeDone(pub std::sync::Arc<Mutex<HashMap<u64, Vec<u64>>>>);
/// v5(M4)：暂停旗标表
#[derive(Clone, Default)]
pub struct ArcPauses(
    pub std::sync::Arc<Mutex<HashMap<u64, std::sync::Arc<std::sync::atomic::AtomicBool>>>>,
);
/// v5(M4)：启动信号表（调度器 → runner）
#[derive(Clone, Default)]
pub struct ArcStarts(pub std::sync::Arc<Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>>);
/// v5(M4)：调度器事件通道（tx/rx 同体；rx 由首次 start 的调度器消费）
#[derive(Clone)]
pub struct ArcDisp(pub std::sync::Arc<Mutex<DispInner>>);
pub struct DispInner {
    pub tx: tokio::sync::mpsc::UnboundedSender<DispMsg>,
    pub rx: Option<tokio::sync::mpsc::UnboundedReceiver<DispMsg>>,
}
impl DispInner {
    pub fn send(&self, m: DispMsg) {
        let _ = self.tx.send(m);
    }
}
#[derive(Debug, Clone, Copy)]
pub enum DispMsg {
    /// 任务入队（id，优先级 rank：0=high 1=normal 2=low）
    Enqueue(u64, u8),
    /// 一个任务结束运行（释放并发槽位）
    Finished,
}

static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

fn is_cancelled(cancels: &ArcCancels, id: u64) -> bool {
    cancels.0.lock().get(&id).map(|c| *c.borrow()).unwrap_or(false)
}

fn emit_progress(app: &AppHandle, p: Value) {
    let _ = app.emit("xfer:progress", p);
}

fn set_status(tasks: &ArcTasks, id: u64, app: &AppHandle, status: &str, extra: Value) {
    let mut m = tasks.0.lock();
    if let Some(t) = m.get_mut(&id) {
        if let Some(o) = t.as_object_mut() {
            o.insert("status".into(), json!(status));
            // 队列模态框字段（phase/bytesDone/bytesTotal/taskId）随状态同步，
            // 与旧 id/status/received/total 并存（sftp.js 两者都消费）
            o.insert("phase".into(), json!(status));
            let r = o.get("received").cloned();
            let t2 = o.get("total").cloned();
            if let (Some(r), Some(t2)) = (r, t2) {
                o.insert("bytesDone".into(), r);
                o.insert("bytesTotal".into(), t2);
            }
            if let Some(iter) = extra.as_object().map(|o| o.iter()) {
                for (k, v) in iter {
                    o.insert(k.clone(), v.clone());
                }
            }
        }
        let snapshot = t.clone();
        emit_progress(app, snapshot);
    }
}

/* ---------- v5(beta) 任务二：纯逻辑已提取至 xfer_core（std-only，供集成测试 #[path] 编入） ---------- */
#[allow(unused_imports)] // pub use 重导出：worker 实现用一部分，单测用另一部分
pub use crate::xfer_core::{
    chunk_delay_ms, per_worker_bps, pick_next, priority_rank, resume_offsets, split_ranges,
    GateAction, worker_gate,
};

/// v5(M4)：优先级感知调度器循环。容量 = 全局并发上限；
/// 入队事件按 pick_next 顺序分配槽位（高优先先跑），Finished 归还槽位后立即补位。
async fn dispatcher_loop(mut rx: tokio::sync::mpsc::UnboundedReceiver<DispMsg>, st: XferState) {
    let mut queue: Vec<(u8, u64)> = Vec::new();
    let mut running: usize = 0;
    while let Some(msg) = rx.recv().await {
        match msg {
            DispMsg::Enqueue(id, rank) => queue.push((rank, id)),
            DispMsg::Finished => running = running.saturating_sub(1),
        }
        while running < st.capacity && !queue.is_empty() {
            let bi = pick_next(&queue).unwrap();
            let (_, id) = queue.remove(bi);
            running += 1;
            let tx = st.starts.0.lock().get(&id).cloned();
            if let Some(tx) = tx {
                let _ = tx.send(true);
            }
        }
    }
}

/* ---------- v5(M2)：续传 journal（跨重启保留断点；解析/upsert 纯逻辑在 xfer_core） ---------- */

fn journal_read() -> Vec<Value> {
    match std::fs::read_to_string(resume_journal_path()) {
        Ok(t) => crate::xfer_core::journal_parse(&t),
        Err(_) => Vec::new(),
    }
}

fn journal_upsert(task: &Value) {
    let new_list = crate::xfer_core::journal_upsert_list(&journal_read(), task);
    let _ = std::fs::write(
        resume_journal_path(),
        serde_json::to_string(&new_list).unwrap_or_else(|_| "[]".into()),
    );
}

/// 发起传输：cfg { sftpId, direction: 'upload'|'download', localPath, remotePath }
/// v5(M2)：pipeline（并行 worker 数，默认 8）、resume（true 时查断点续传）
/// v5(M4)：priority（'high'|'normal'|'low'，默认 normal；高优先先分配并发槽位）
#[allow(clippy::too_many_arguments)]
pub async fn start(
    state: XferState,
    app: AppHandle,
    direction: String,
    sftp_id: u64,
    local_path: String,
    remote_path: String,
    pipeline: Option<usize>,
    resume: Option<bool>,
    priority: Option<String>,
    // v5(M4.1)/F-M4-02：单任务限速（字节/秒，0 或缺省=不限；按 worker 数均分）
    speed_limit: Option<u64>,
) -> Result<Value, String> {
    // v5(M4)：调度器首次使用时惰性拉起（单实例，XferState 克隆共享同一事件通道）
    if !state.disp_started.swap(true, Ordering::SeqCst) {
        let st = state.clone();
        let rx = state.disp.0.lock().rx.take();
        if let Some(rx) = rx {
            tokio::spawn(dispatcher_loop(rx, st));
        }
    }
    let sftp = crate::sftp::get(&state.sftps, sftp_id)?;
    let workers = pipeline
        .unwrap_or(DEFAULT_PIPELINE)
        .clamp(1, MAX_PIPELINE);
    // v4.0.10 修复：上传时远端文件尚不存在，不能调用远端 metadata（会报"no such file"
    // 导致上传无反应）——上传取本地文件大小，下载取远端大小
    let total: u64 = if direction == "upload" {
        tokio::fs::metadata(&local_path)
            .await
            .map(|m| m.len())
            .map_err(|e| format!("读取本地文件失败: {e}"))?
    } else {
        sftp
            .metadata(&remote_path)
            .await
            .map(|m| m.size.unwrap_or(0))
            .map_err(|e| format!("读取远端信息失败: {e}"))?
    };

    let id = NEXT_TASK.fetch_add(1, Ordering::SeqCst);
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    state.cancels.0.lock().insert(id, cancel_tx);

    // ---------- v5(M2)：断点续传决策 ----------
    let mut resumed_from: u64 = 0;
    let mut range_done: Option<Vec<u64>> = None;
    if resume == Some(true) {
        // 先查内存任务表（本次运行内的失败/取消任务），再查磁盘 journal（跨重启）
        let prev = {
            let m = state.tasks.0.lock();
            m.values()
                .filter(|t| {
                    t.get("direction").and_then(|x| x.as_str()) == Some(direction.as_str())
                        && t.get("localPath").and_then(|x| x.as_str()) == Some(local_path.as_str())
                        && t.get("remotePath").and_then(|x| x.as_str()) == Some(remote_path.as_str())
                        && matches!(
                            t.get("status").and_then(|x| x.as_str()),
                            Some("failed") | Some("cancelled")
                        )
                })
                .max_by_key(|t| t.get("id").and_then(|x| x.as_u64()).unwrap_or(0))
                .cloned()
        };
        let prev = prev.or_else(|| {
            journal_read()
                .into_iter()
                .find(|t| {
                    t.get("direction").and_then(|x| x.as_str()) == Some(direction.as_str())
                        && t.get("localPath").and_then(|x| x.as_str()) == Some(local_path.as_str())
                        && t.get("remotePath").and_then(|x| x.as_str()) == Some(remote_path.as_str())
                })
        });
        if let Some(p) = prev {
            let rd: Option<Vec<u64>> = p
                .get("rangeDone")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_u64()).collect());
            let rec_total = p.get("total").and_then(|x| x.as_u64()).unwrap_or(0);
            if rec_total == total {
                match direction.as_str() {
                    "download" => {
                        // 远端大小没变；本地已有部分大小须 >= 每 worker 断点
                        let local_len = tokio::fs::metadata(&local_path)
                            .await
                            .map(|m| m.len())
                            .unwrap_or(0);
                        let ranges = split_ranges(0, total, workers);
                        let offs = resume_offsets(rd.as_deref(), &ranges);
                        let max_off = offs.iter().copied().max().unwrap_or(0);
                        if total > 0 && local_len >= max_off {
                            range_done = rd;
                            resumed_from = offs.iter().sum();
                        } else if total > 0 {
                            return Err("本地文件不完整且与断点不符,建议重新下载(整文件)".into());
                        }
                    }
                    _ => {
                        // 上传：远端已有部分大小必须 <= 本地文件大小，否则远端已被改写过
                        let remote_size = sftp
                            .metadata(&remote_path)
                            .await
                            .map(|m| m.size.unwrap_or(0))
                            .unwrap_or(0);
                        crate::xfer_core::resume_upload_guard(remote_size, total)?;
                        let ranges = split_ranges(0, total, workers);
                        let offs = resume_offsets(rd.as_deref(), &ranges);
                        range_done = rd;
                        resumed_from = offs.iter().sum::<u64>().min(total);
                    }
                }
            } else {
                return Err("源文件大小与断点记录不一致,建议重传".into());
            }
        } else {
            return Err("没有可续传的历史任务".into());
        }
    }

    let prio = priority.unwrap_or_else(|| "normal".to_string());
    let prio_rank = priority_rank(&prio);
    let task = json!({
        "id": id,
        "taskId": id,
        "sftpId": sftp_id,
        "direction": direction,
        "localPath": local_path,
        "remotePath": remote_path,
        "total": total,
        "received": resumed_from,
        "bytesDone": resumed_from,
        "bytesTotal": total,
        "pipeline": workers,
        "resumedFrom": resumed_from,
        "priority": prio,
        "status": if resumed_from > 0 { "resuming" } else { "queued" },
        "phase": if resumed_from > 0 { "resuming" } else { "queued" },
    });
    state.tasks.0.lock().insert(id, task.clone());
    emit_progress(&app, task);

    // v5(M4)：调度启动信号 + 入队（由优先级调度器分配并发槽位后放行）
    let (start_tx, mut start_rx) = tokio::sync::watch::channel(false);
    state.starts.0.lock().insert(id, start_tx);
    state.disp.0.lock().send(DispMsg::Enqueue(id, prio_rank));

    // 后台执行
    let st = state.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        // v5(M4)：等待调度器放行（高优先先分配）；排队期间可被取消
        let mut granted = false;
        let mut cancel_rx = cancel_rx;
        loop {
            tokio::select! {
                _ = start_rx.changed() => { if *start_rx.borrow() { granted = true; break; } }
                _ = cancel_rx.changed() => { if *cancel_rx.borrow() { break; } }
            }
        }
        let was_granted = *start_rx.borrow();
        if !granted {
            // 排队/放行竞态中被取消：不进入运行态；若调度器已计数则归还槽位
            st.cancels.0.lock().remove(&id);
            st.starts.0.lock().remove(&id);
            st.pauses.0.lock().remove(&id);
            if was_granted {
                st.disp.0.lock().send(DispMsg::Finished);
            }
            set_status(&st.tasks, id, &app2, "cancelled", json!({}));
            return;
        }
        let pause_flag = st.pauses.0.lock().get(&id).cloned();
        set_status(&st.tasks, id, &app2, "running", json!({}));
        let cancelled;

        // v5(M2)：共享完成字节数（含续传起点），worker 各自累加
        let done = std::sync::Arc::new(AtomicU64::new(resumed_from));

        let started = std::time::Instant::now();
        // 进度播报任务：100ms 间隔读共享计数推送（事件协议与旧版兼容）
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let t_done = done.clone();
        let t_app = app2.clone();
        let sid = id;
        let tot = total;
        let t_prio = prio.clone();
        let t_pause = pause_flag.clone();
        let ticker = tokio::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_millis(100));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let t0 = std::time::Instant::now();
            loop {
                tokio::select! {
                    _ = iv.tick() => {
                        let d = t_done.load(Ordering::SeqCst);
                        let paused = t_pause
                            .as_ref()
                            .map(|p| p.load(Ordering::SeqCst))
                            .unwrap_or(false);
                        let stat = if paused { "paused" } else { "running" };
                        let elapsed = t0.elapsed().as_secs_f64();
                        let bps = if !paused && elapsed > 0.2 { (d as f64 / elapsed) as u64 } else { 0 };
                        let _ = t_app.emit("xfer:progress", json!({
                            "id": sid, "taskId": sid,
                            "status": stat, "phase": stat,
                            "priority": t_prio,
                            "received": d, "total": tot,
                            "bytesDone": d, "bytesTotal": tot,
                            "speedBps": bps,
                        }));
                    }
                    _ = stop_rx.changed() => break,
                    else => break,
                }
            }
        });
        let result: Result<(), String> = run_transfer(
            &st, id, sftp_id, &direction, &local_path, &remote_path, total, workers,
            range_done.as_deref(), resumed_from, &done, cancel_rx.clone(), pause_flag.as_ref(),
            chunk_delay_ms(speed_limit.unwrap_or(0), workers, CHUNK),
        )
        .await;
        let _ = stop_tx.send(true);
        let _ = ticker.await;

        cancelled = is_cancelled(&st.cancels, id);
        st.cancels.0.lock().remove(&id);
        // v5(M4)：运行结束清理暂停/启动信号表，归还调度槽位
        st.pauses.0.lock().remove(&id);
        st.starts.0.lock().remove(&id);
        st.disp.0.lock().send(DispMsg::Finished);

        // 记录每个 worker 的已传偏移（供续传决策）
        let cur_done = done.load(Ordering::SeqCst);
        if let Some(rd) = st.range_done.0.lock().remove(&id) {
            let mut m = st.tasks.0.lock();
            if let Some(t) = m.get_mut(&id) {
                if let Some(o) = t.as_object_mut() {
                    o.insert("rangeDone".into(), json!(rd));
                    o.insert("received".into(), json!(cur_done));
                    o.insert("bytesDone".into(), json!(cur_done));
                }
            }
            // 失败/取消的可续传任务写入 journal（跨重启保留断点）
            let snap = m.get(&id).cloned();
            drop(m);
            if let Some(t) = snap {
                journal_upsert(&t);
            }
        }

        if cancelled {
            set_status(&st.tasks, id, &app2, "cancelled", json!({}));
        } else if let Err(e) = result {
            set_status(&st.tasks, id, &app2, "failed", json!({ "error": e }));
        } else {
            // 成功后清掉该文件的续传记录（received=0 → journal_upsert 仅做移除）
            journal_upsert(&json!({
                "direction": direction, "localPath": local_path,
                "remotePath": remote_path, "total": total, "received": 0,
            }));
            let elapsed = started.elapsed().as_secs_f64();
            set_status(&st.tasks, id, &app2, "done", json!({
                "received": total, "bytesDone": total,
                "speedBps": if elapsed > 0.0 { (total as f64 / elapsed) as u64 } else { 0 },
            }));
        }
    });

    Ok(json!({ "ok": true, "id": id, "taskId": id, "status": "queued", "received": resumed_from, "total": total }))
}

/// v5(M2) 传输执行体：把 [0,total) 切成 workers 个区间并行推进。
/// range_done：续传时每个 worker 的起点相对区间起点的偏移（None = 全量）。
/// 结束后（无论成败）把每个 worker 的实际已传偏移写入 st.range_done[id]。
#[allow(clippy::too_many_arguments)]
async fn run_transfer(
    st: &XferState,
    id: u64,
    sftp_id: u64,
    direction: &str,
    local_path: &str,
    remote_path: &str,
    total: u64,
    workers: usize,
    range_done: Option<&[u64]>,
    resumed_from: u64,
    done: &std::sync::Arc<AtomicU64>,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    // v5(M4)：暂停旗标（None = 不支持暂停的调用路径，恒不暂停）
    pause: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
    // v5(M4.1)：每 worker 块间延迟（毫秒，0=不限速）
    chunk_delay_ms: u64,
) -> Result<(), String> {
    let sftp = crate::sftp::get(&st.sftps, sftp_id)?;
    if total == 0 {
        return Ok(());
    }
    let ranges = split_ranges(0, total, workers.min(total as usize));
    let mut offs = resume_offsets(range_done, &ranges);
    if resumed_from == 0 {
        offs = ranges.iter().map(|(s, _)| *s).collect();
    }

    // v5(M2) 多 worker 上传：先同步 truncate 远端文件再分发 worker，
    // 避免 worker0 的 TRUNCATE 与其他 worker 的写入竞态
    if direction == "upload" && resumed_from == 0 {
        let _ = sftp
            .open_with_flags(remote_path, OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE)
            .await
            .map_err(|e| format!("打开远端文件失败: {e}"))?;
    }

    // 每 worker 独立计数（相对区间起点），取消/失败时即为精确断点
    let per_worker: Vec<std::sync::Arc<AtomicU64>> = (0..ranges.len())
        .map(|_| std::sync::Arc::new(AtomicU64::new(0)))
        .collect();
    let pause_owned: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = pause.cloned();

    let spawn = |set: &mut tokio::task::JoinSet<Result<(), String>>, i: usize| {
        let sftp = sftp.clone();
        let (rs, re) = ranges[i];
        let off = offs[i];
        let dir = direction.to_string();
        let lp = local_path.to_string();
        let rp = remote_path.to_string();
        let done_c = done.clone();
        let wc = per_worker[i].clone();
        let mut cancel = cancel_rx.clone();
        let pz = pause_owned.clone();
        let dms = chunk_delay_ms;
        set.spawn(async move {
            // worker 同时累加「区间内计数 wc」与「全局进度 done」（避免聚合延迟）
            let r = if dir == "upload" {
                upload_worker(&sftp, &lp, &rp, rs, re, off, &wc, &done_c, &mut cancel, pz.as_deref(), dms).await
            } else {
                download_worker(&sftp, &lp, &rp, rs, re, off, &wc, &done_c, &mut cancel, pz.as_deref(), dms).await
            };
            match r {
                Err(e) if e == "已取消" => Ok(()), // 取消不视为错误（外层按 watch 判定）
                other => other,
            }
        });
    };

    let mut set: tokio::task::JoinSet<Result<(), String>> = tokio::task::JoinSet::new();
    for i in 0..ranges.len() {
        spawn(&mut set, i);
    }
    let mut first_err: Option<String> = None;
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                    // 一个 worker 失败 → 终止其余，保留各 worker 断点
                    set.abort_all();
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(format!("传输线程异常: {e}"));
                    set.abort_all();
                }
            }
        }
    }
    // 精确断点：每 worker 已传字节数（相对区间起点）
    let rd: Vec<u64> = per_worker.iter().map(|w| w.load(Ordering::SeqCst)).collect();
    st.range_done.0.lock().insert(id, rd);
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/* ---------- v5(M2)：worker 实现 ---------- */

#[allow(clippy::too_many_arguments)]
async fn upload_worker(
    sftp: &SftpSession,
    local_path: &str,
    remote_path: &str,
    start: u64,
    end: u64,
    off: u64,
    wc: &AtomicU64,
    done: &AtomicU64,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    // v5(M4)：Some(旗标) 时支持暂停；None 恒不暂停
    pause: Option<&std::sync::atomic::AtomicBool>,
    // v5(M4.1)：每 worker 块间延迟（毫秒，0=不限速）；暂停/取消门控不受影响
    delay_ms: u64,
) -> Result<(), String> {
    let mut lf = tokio::fs::File::open(local_path)
        .await
        .map_err(|e| format!("打开本地文件失败: {e}"))?;
    let mut rf = sftp
        .open_with_flags(remote_path, OpenFlags::WRITE | OpenFlags::CREATE)
        .await
        .map_err(|e| format!("打开远端文件失败: {e}"))?;
    let mut pos = off;
    if pos > 0 {
        rf.seek(std::io::SeekFrom::Start(pos))
            .await
            .map_err(|e| format!("定位远端偏移失败: {e}"))?;
        lf.seek(std::io::SeekFrom::Start(pos))
            .await
            .map_err(|e| format!("定位本地偏移失败: {e}"))?;
    }
    let mut buf = vec![0u8; CHUNK as usize];
    while pos < end {
        // v5(M4) 门控：取消 > 暂停 > 推进（暂停时在途块已写完，停等不发起新请求）
        // （先把两个旗标读成本地值，避免 watch 借用守卫跨 await）
        let paused_now = pause.map(|p| p.load(Ordering::SeqCst)).unwrap_or(false);
        let cancelled_now = *cancel.borrow();
        match worker_gate(paused_now, cancelled_now) {
            GateAction::Advance => {}
            GateAction::Cancelled => return Err("已取消".into()),
            GateAction::WaitPaused => {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                continue;
            }
        }
        let want = (end - pos).min(CHUNK) as usize;
        let read = read_exact_async(&mut lf, &mut buf[..want]).await?;
        if read == 0 {
            return Err("本地文件比预期短".into());
        }
        rf.write_all(&buf[..read])
            .await
            .map_err(|e| format!("写入远端失败: {e}"))?;
        pos += read as u64;
        wc.fetch_add(read as u64, Ordering::SeqCst);
        done.fetch_add(read as u64, Ordering::SeqCst);
        // v5(M4.1) 限速：块间节流（sleep 在门控之后，暂停/取消语义不受限速影响）
        if delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
    }
    let _ = start;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn download_worker(
    sftp: &SftpSession,
    local_path: &str,
    remote_path: &str,
    start: u64,
    end: u64,
    off: u64,
    wc: &AtomicU64,
    done: &AtomicU64,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    pause: Option<&std::sync::atomic::AtomicBool>,
    // v5(M4.1)：每 worker 块间延迟（毫秒，0=不限速）
    delay_ms: u64,
) -> Result<(), String> {
    let mut rf = sftp
        .open(remote_path)
        .await
        .map_err(|e| format!("打开远端文件失败: {e}"))?;
    // 本地文件：首次全量下载时创建；续传时以写方式打开（不 truncate）
    let mut lf = if off > 0 {
        tokio::fs::OpenOptions::new()
            .write(true)
            .open(local_path)
            .await
            .map_err(|e| format!("打开本地文件失败: {e}"))?
    } else {
        tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(local_path)
            .await
            .map_err(|e| format!("创建本地文件失败: {e}"))?
    };
    let mut pos = off;
    if pos > 0 {
        rf.seek(std::io::SeekFrom::Start(pos))
            .await
            .map_err(|e| format!("定位远端偏移失败: {e}"))?;
        lf.seek(std::io::SeekFrom::Start(pos))
            .await
            .map_err(|e| format!("定位本地偏移失败: {e}"))?;
    }
    let mut buf = vec![0u8; CHUNK as usize];
    while pos < end {
        // v5(M4) 门控：取消 > 暂停 > 推进（暂停时在途块已写完，停等不发起新请求）
        // （先把两个旗标读成本地值，避免 watch 借用守卫跨 await）
        let paused_now = pause.map(|p| p.load(Ordering::SeqCst)).unwrap_or(false);
        let cancelled_now = *cancel.borrow();
        match worker_gate(paused_now, cancelled_now) {
            GateAction::Advance => {}
            GateAction::Cancelled => return Err("已取消".into()),
            GateAction::WaitPaused => {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                continue;
            }
        }
        let want = (end - pos).min(CHUNK) as usize;
        let n = rf
            .read(&mut buf[..want])
            .await
            .map_err(|e| format!("读取远端失败: {e}"))?;
        if n == 0 {
            return Err("远端文件比预期短".into());
        }
        lf.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
        pos += n as u64;
        wc.fetch_add(n as u64, Ordering::SeqCst);
        done.fetch_add(n as u64, Ordering::SeqCst);
        // v5(M4.1) 限速：块间节流（暂停/取消门控在循环顶部，语义不受限速影响）
        if delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
    }
    let _ = lf.flush().await;
    let _ = start;
    Ok(())
}

/// read_exact 但容忍 EOF 提前结束（返回实际读取量）
async fn read_exact_async<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut [u8],
) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r
            .read(&mut buf[filled..])
            .await
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

pub async fn cancel(state: XferState, task_id: u64) -> Result<Value, String> {
    if let Some(tx) = state.cancels.0.lock().get(&task_id) {
        let _ = tx.send(true);
        Ok(json!({ "ok": true }))
    } else {
        Err("任务不存在".into())
    }
}

/// v5(M4)：暂停任务——worker 传完在途块后停等；断点保留在内存，任务保活。
/// 语义与取消区分：取消=终止任务（断点写 journal，可▶续传重建）；暂停=任务仍在运行态。
pub fn pause(state: &XferState, app: &AppHandle, task_id: u64) -> Result<Value, String> {
    if !state.cancels.0.lock().contains_key(&task_id) {
        return Err("任务不存在或已结束".into());
    }
    let flag = state
        .pauses
        .0
        .lock()
        .entry(task_id)
        .or_insert_with(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .clone();
    flag.store(true, Ordering::SeqCst);
    // 状态即时推送 paused（worker 完成在途块后实际停等；速率随 ticker 归零）
    set_status(&state.tasks, task_id, app, "paused", json!({}));
    Ok(json!({ "ok": true, "status": "paused" }))
}

/// v5(M4)：恢复已暂停任务——从暂停处（各 worker 断点）继续
pub fn resume(state: &XferState, app: &AppHandle, task_id: u64) -> Result<Value, String> {
    let flag = {
        let m = state.pauses.0.lock();
        m.get(&task_id).cloned()
    };
    let Some(flag) = flag else {
        return Err("任务不存在或未暂停".into());
    };
    flag.store(false, Ordering::SeqCst);
    set_status(&state.tasks, task_id, app, "running", json!({}));
    Ok(json!({ "ok": true, "status": "running" }))
}

/// 任务列表（渲染端刷新队列用）
pub fn list(state: &XferState) -> Vec<Value> {
    state.tasks.0.lock().values().cloned().collect()
}

/// 目录下载：递归遍历远端目录逐文件入队（3.x 同名功能）
pub async fn download_dir(
    state: XferState,
    app: AppHandle,
    sftp_id: u64,
    remote_dir: String,
    local_dir: PathBuf,
) -> Result<u32, String> {
    let sftp = crate::sftp::get(&state.sftps, sftp_id)?;
    let entries = sftp
        .read_dir(&remote_dir)
        .await
        .map_err(|e| format!("读取目录失败: {e}"))?;
    std::fs::create_dir_all(&local_dir).map_err(|e| format!("创建本地目录失败: {e}"))?;
    let mut count = 0u32;
    for e in entries {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let md = e.metadata();
        let rpath = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
        let lpath = local_dir.join(&name);
        if md.is_dir() {
            let n = Box::pin(download_dir(state.clone(), app.clone(), sftp_id, rpath, lpath)).await?;
            count += n;
        } else {
            start(
                state.clone(),
                app.clone(),
                "download".into(),
                sftp_id,
                lpath.to_string_lossy().to_string(),
                rpath,
                None,
                None,
                None,
                None,
            )
            .await?;
            count += 1;
        }
    }
    Ok(count)
}

/* ---------- 单元测试 ---------- */
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_ranges_basic() {
        assert_eq!(split_ranges(0, 10, 3), vec![(0, 4), (4, 7), (7, 10)]);
        assert!(split_ranges(0, 0, 8).is_empty());
        // worker 数多于字节数时收缩
        assert_eq!(split_ranges(0, 2, 8), vec![(0, 1), (1, 2)]);
        // 单区间
        assert_eq!(split_ranges(5, 9, 1), vec![(5, 9)]);
    }

    #[test]
    fn resume_offsets_match() {
        let ranges = split_ranges(0, 100, 4); // (0,25)(25,50)(50,75)(75,100)
        let rd = vec![10, 25, 0, 5];
        let offs = resume_offsets(Some(&rd), &ranges);
        assert_eq!(offs, vec![10, 50, 50, 80]);
    }

    #[test]
    fn resume_offsets_mismatch_falls_back() {
        let ranges = split_ranges(0, 100, 4);
        // worker 数不一致 → 全量
        let offs = resume_offsets(Some(&[1u64, 2]), &ranges);
        assert_eq!(offs, vec![0, 25, 50, 75]);
        let offs = resume_offsets(None, &ranges);
        assert_eq!(offs, vec![0, 25, 50, 75]);
    }

    /* ---------- v5(M4)：暂停/恢复状态机 ---------- */

    #[test]
    fn worker_gate_pause_state_machine() {
        use GateAction::*;
        // 正常：推进
        assert_eq!(worker_gate(false, false), Advance);
        // 暂停请求：停等（不发起新块）
        assert_eq!(worker_gate(true, false), WaitPaused);
        // 取消优先于暂停：立即终止
        assert_eq!(worker_gate(true, true), Cancelled);
        assert_eq!(worker_gate(false, true), Cancelled);
        // 优先级 rank：high < normal < low
        assert!(priority_rank("high") < priority_rank("normal"));
        assert!(priority_rank("normal") < priority_rank("low"));
        // 未知值按 normal
        assert_eq!(priority_rank("bogus"), priority_rank("normal"));
        assert_eq!(priority_rank(""), priority_rank("normal"));
    }

    #[test]
    fn pause_flag_transitions_preserve_breakpoint() {
        // 暂停→恢复只翻转旗标，不触碰 per-worker 断点：
        // 模拟 4 worker 各传了若干块后暂停（断点=当前计数），恢复后断点原样可用。
        let ranges = split_ranges(0, 400, 4); // (0,100)(100,200)(200,300)(300,400)
        let partial: Vec<u64> = vec![64, 100, 32, 0]; // 每 worker 已传（块对齐模拟）
        let pause_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // 暂停
        pause_flag.store(true, Ordering::SeqCst);
        assert_eq!(pause_flag.load(Ordering::SeqCst), true);
        // 恢复
        pause_flag.store(false, Ordering::SeqCst);
        assert_eq!(pause_flag.load(Ordering::SeqCst), false);
        // 断点折算 + 续传起点：恢复后各 worker 从断点继续（roundtrip 一致性）
        let offs_before = resume_offsets(Some(&partial), &ranges);
        // 再传一轮后再次暂停：新断点 = 旧断点继续，resume_offsets 对新断点仍给出正确起点
        let grown: Vec<u64> = partial
            .iter()
            .enumerate()
            .map(|(i, d)| (d + 16).min(ranges[i].1 - ranges[i].0))
            .collect();
        let offs_after = resume_offsets(Some(&grown), &ranges);
        for (i, ((s, _), ob)) in ranges.iter().zip(offs_before.iter()).enumerate() {
            assert_eq!(*s + grown[i], offs_after[i]);
            assert!(offs_after[i] >= *ob); // 只前进不回退
        }
    }

    #[test]
    fn scheduler_priority_ordering() {
        // 高优先先分配；同优先级按入队顺序（id 小者先）
        let q = vec![(1u8, 3u64), (0, 7), (2, 1), (0, 2), (1, 5)];
        let mut order = Vec::new();
        let mut qq = q.clone();
        while let Some(i) = pick_next(&qq) {
            order.push(qq.remove(i));
        }
        assert_eq!(order, vec![(0, 2), (0, 7), (1, 3), (1, 5), (2, 1)]);
        // 空队列 → None
        assert_eq!(pick_next(&[]), None);
        // FIFO 保证：即使后入队的高优先 id 更大，也先于普通任务
        let qq = vec![(1, 1), (0, 9)];
        assert_eq!(pick_next(&qq), Some(1));
    }

    /* ---------- v5(M4.1)：单任务限速节流 ---------- */

    #[test]
    fn speed_limit_delay_derivation() {
        // 不限速：0 → 延迟 0
        assert_eq!(chunk_delay_ms(0, 8, CHUNK), 0);
        assert_eq!(per_worker_bps(0, 8), 0);
        // 10MB/s、8 worker → 每 worker 1.25MB/s，1MiB 块间约 838ms
        let d = chunk_delay_ms(10 * 1024 * 1024, 8, CHUNK);
        assert_eq!(per_worker_bps(10 * 1024 * 1024, 8), (10 * 1024 * 1024) / 8);
        assert_eq!(d, (CHUNK * 1000) / ((10 * 1024 * 1024) / 8));
        // 限速均分：worker 数翻倍 → 每 worker 延迟翻倍（聚合速率不变）
        let d1 = chunk_delay_ms(1024 * 1024, 1, CHUNK); // 1 worker：1MiB/1MiBps → 1000ms
        let d2 = chunk_delay_ms(1024 * 1024, 2, CHUNK); // 2 worker：各 0.5MiB/s → 2000ms
        assert_eq!(d1, 1000);
        assert_eq!(d2, 2000);
        // 极小限速：每 worker 不足 1 B/s 时钳到 1，延迟钳 60s 防挂死
        let dmax = chunk_delay_ms(1, 16, CHUNK);
        assert_eq!(dmax, 60_000);
        // 大限速：延迟向下取整可为 0（等价于不限速的块间节奏）
        assert_eq!(chunk_delay_ms(u64::MAX, 1, CHUNK), 0);
    }

    #[test]
    fn speed_limit_overlay_with_pause() {
        // 限速与暂停叠加：暂停旗标置位时，worker 门控返回 WaitPaused（不发起新块），
        // 限速 sleep 只发生在「已推进一块之后」——即暂停优先于限速节流；
        // 恢复后（旗标清零）按限速延迟继续推进。
        let limit = 2 * 1024 * 1024; // 2MiB/s
        let workers = 4usize;
        let dms = chunk_delay_ms(limit, workers, CHUNK);
        assert_eq!(dms, (CHUNK * 1000) / (limit / workers as u64)); // 1MiB*1000/512KiB = 2000ms
        let pause_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        // 暂停中：无论是否限速，门控结果都是 WaitPaused（限速不吞掉暂停）
        let paused_now = pause_flag.load(Ordering::SeqCst);
        assert_eq!(worker_gate(paused_now, false), GateAction::WaitPaused);
        // 恢复：门控回到 Advance，下一块后按 dms 节流
        pause_flag.store(false, Ordering::SeqCst);
        assert_eq!(worker_gate(false, false), GateAction::Advance);
        assert!(dms > 0);
        // 取消仍优先于暂停/限速路径
        assert_eq!(worker_gate(true, true), GateAction::Cancelled);
    }
}
