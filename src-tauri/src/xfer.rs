/* EasySSH v4：SFTP 传输队列（对应 3.x xfer:* 命令）
 * v5(M2)/F-M2-01 传输管道化：大文件读写按范围切分为 N 个并行 worker
 *   （默认 8，可配 1..=16），每个 worker 以 1MiB 块顺序推进自己的区间，
 *   在途请求 = worker 数（上传时 russh-sftp File 内部还有写确认流水线），
 *   千兆局域网/高延迟链路吞吐相比单请求串行可提升数倍。
 * v5(M2)/F-M2-02 断点续传：任务失败/取消时按 worker 记录已传偏移
 *   （内存 tasks 表 + 磁盘 xfer-resume.json 双份），重传时带 resume=true
 *   即从断点继续；续传前校验远端/本地已有部分大小。
 * 进度仍经 "xfer:progress" 事件推送（协议向后兼容：原 id/status/received/total
 * 字段不变，另附队列模态框所需的 taskId/phase/bytesDone/bytesTotal 等字段）。 */
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// v4.3.1 效率强化：并发闸门（默认 4）——目录批量下载等场景旧实现任务数无上限
    pub gate: std::sync::Arc<tokio::sync::Semaphore>,
    /// v5(M2)：任务 id → 各 worker 已传偏移（相对区间起点）；失败/取消时折入任务记录
    pub range_done: ArcRangeDone,
}

#[derive(Clone, Default)]
pub struct ArcTasks(pub std::sync::Arc<Mutex<HashMap<u64, Value>>>);
#[derive(Clone, Default)]
pub struct ArcCancels(pub std::sync::Arc<Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>>);
#[derive(Clone, Default)]
pub struct ArcRangeDone(pub std::sync::Arc<Mutex<HashMap<u64, Vec<u64>>>>);

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

/* ---------- v5(M2)：范围切分与续传辅助（纯函数，便于单测） ---------- */

/// 把 [start, end) 切成 n 个尽量均匀的连续区间
pub fn split_ranges(start: u64, end: u64, n: usize) -> Vec<(u64, u64)> {
    let n = n.max(1);
    let total = end.saturating_sub(start);
    if total == 0 {
        return Vec::new();
    }
    let n = n.min(total as usize);
    let base = total / n as u64;
    let rem = total % n as u64;
    let mut out = Vec::with_capacity(n);
    let mut cur = start;
    for i in 0..n {
        let len = base + if (i as u64) < rem { 1 } else { 0 };
        out.push((cur, cur + len));
        cur += len;
    }
    out
}

/// 续传起始偏移决策：
/// - 断点记录里的 rangeDone 与本次 worker 数一致 → 每个 worker 从各自断点继续
/// - 否则退化为不续传（offset = 区间起点，覆盖语义不变）
pub fn resume_offsets(range_done: Option<&[u64]>, ranges: &[(u64, u64)]) -> Vec<u64> {
    match range_done {
        Some(v) if v.len() == ranges.len() && !v.is_empty() => ranges
            .iter()
            .zip(v.iter())
            .map(|((s, e), d)| (*s + d).min(*e))
            .collect(),
        _ => ranges.iter().map(|(s, _)| *s).collect(),
    }
}

/* ---------- v5(M2)：续传 journal（跨重启保留断点） ---------- */

fn journal_read() -> Vec<Value> {
    let txt = match std::fs::read_to_string(resume_journal_path()) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&txt).unwrap_or_default()
}

fn journal_upsert(task: &Value) {
    let mut list = journal_read();
    let dir = task.get("direction").and_then(|x| x.as_str()).unwrap_or("");
    let lp = task.get("localPath").and_then(|x| x.as_str()).unwrap_or("");
    let rp = task.get("remotePath").and_then(|x| x.as_str()).unwrap_or("");
    list.retain(|t| {
        !(t.get("direction").and_then(|x| x.as_str()) == Some(dir)
            && t.get("localPath").and_then(|x| x.as_str()) == Some(lp)
            && t.get("remotePath").and_then(|x| x.as_str()) == Some(rp))
    });
    if task.get("received").and_then(|x| x.as_u64()).unwrap_or(0) > 0 {
        list.push(task.clone());
    }
    let _ = std::fs::write(
        resume_journal_path(),
        serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()),
    );
}

/// 发起传输：cfg { sftpId, direction: 'upload'|'download', localPath, remotePath }
/// v5(M2)：pipeline（并行 worker 数，默认 8）、resume（true 时查断点续传）
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
) -> Result<Value, String> {
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
                        if remote_size > total {
                            return Err("远端文件已变化(比本地大),建议重传".into());
                        }
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
        "status": if resumed_from > 0 { "resuming" } else { "queued" },
        "phase": if resumed_from > 0 { "resuming" } else { "queued" },
    });
    state.tasks.0.lock().insert(id, task.clone());
    emit_progress(&app, task);

    // 后台执行
    let st = state.clone();
    let app2 = app.clone();
    tokio::spawn(async move {
        // v4.3.1 并发闸门：超出 4 个并发任务时在此排队，避免带宽/内存被挤占
        let _permit = st.gate.acquire().await;
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
        let ticker = tokio::spawn(async move {
            let mut iv = tokio::time::interval(std::time::Duration::from_millis(100));
            iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let t0 = std::time::Instant::now();
            loop {
                tokio::select! {
                    _ = iv.tick() => {
                        let d = t_done.load(Ordering::SeqCst);
                        let elapsed = t0.elapsed().as_secs_f64();
                        let bps = if elapsed > 0.2 { (d as f64 / elapsed) as u64 } else { 0 };
                        let _ = t_app.emit("xfer:progress", json!({
                            "id": sid, "taskId": sid,
                            "status": "running", "phase": "running",
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
            range_done.as_deref(), resumed_from, &done, cancel_rx.clone(),
        )
        .await;
        let _ = stop_tx.send(true);
        let _ = ticker.await;

        cancelled = is_cancelled(&st.cancels, id);
        st.cancels.0.lock().remove(&id);

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
        set.spawn(async move {
            // worker 同时累加「区间内计数 wc」与「全局进度 done」（避免聚合延迟）
            let r = if dir == "upload" {
                upload_worker(&sftp, &lp, &rp, rs, re, off, &wc, &done_c, &mut cancel).await
            } else {
                download_worker(&sftp, &lp, &rp, rs, re, off, &wc, &done_c, &mut cancel).await
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
        if *cancel.borrow() {
            return Err("已取消".into());
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
        if *cancel.borrow() {
            return Err("已取消".into());
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
}
