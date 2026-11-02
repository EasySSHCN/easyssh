/* EasySSH v4：SFTP 传输队列（对应 3.x xfer:* 命令）
 * 每个传输任务一个 tokio 任务：流式读远端 File ↔ 本地文件，进度经
 * "xfer:progress" 事件推送；支持取消（watch 信号）。 */
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use russh_sftp::protocol::OpenFlags;
use serde_json::Value;

use parking_lot::Mutex;
use russh_sftp::client::SftpSession;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct XferState {
    pub sftps: crate::sftp::SftpMap,
    pub tasks: ArcTasks,
    pub cancels: ArcCancels,
    /// v4.3.1 效率强化：并发闸门（默认 4）——目录批量下载等场景旧实现任务数无上限
    pub gate: std::sync::Arc<tokio::sync::Semaphore>,
}

#[derive(Clone, Default)]
pub struct ArcTasks(pub std::sync::Arc<Mutex<HashMap<u64, Value>>>);
#[derive(Clone, Default)]
pub struct ArcCancels(pub std::sync::Arc<Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>>);

static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

fn emit_progress(app: &AppHandle, p: Value) {
    let _ = app.emit("xfer:progress", p);
}

fn set_status(tasks: &ArcTasks, id: u64, app: &AppHandle, status: &str, extra: Value) {
    let mut m = tasks.0.lock();
    if let Some(t) = m.get_mut(&id) {
        if let Some(o) = t.as_object_mut() {
            o.insert("status".into(), json!(status));
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

/// 发起传输：cfg { sftpId, direction: 'upload'|'download', localPath, remotePath }
#[allow(clippy::too_many_arguments)]
pub async fn start(
    state: XferState,
    app: AppHandle,
    direction: String,
    sftp_id: u64,
    local_path: String,
    remote_path: String,
) -> Result<Value, String> {
    let sftp = crate::sftp::get(&state.sftps, sftp_id)?;
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

    let task = json!({
        "id": id,
        "taskId": id,
        "sftpId": sftp_id,
        "direction": direction,
        "localPath": local_path,
        "remotePath": remote_path,
        "total": total,
        "received": 0,
        "status": "queued",
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
        let mut cancelled = false;
        // v4.3.0 效率强化：进度事件节流——100ms 或 1% 进度增量才推送一次，
        // 避免 256KB 粒度的逐块 emit 在大文件传输时形成事件风暴。
        const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
        let mut last_emit = std::time::Instant::now() - PROGRESS_INTERVAL;
        let mut last_emitted: u64 = 0;
        let mut emit_throttled = |done: u64, total: u64, app: &AppHandle, sid: u64| {
            let now = std::time::Instant::now();
            let pct_moved = total > 0
                && (done.saturating_sub(last_emitted)) * 100 / total >= 1;
            if now.duration_since(last_emit) >= PROGRESS_INTERVAL || pct_moved {
                last_emit = now;
                last_emitted = done;
                emit_progress(app, json!({ "id": sid, "status": "running", "received": done, "total": total }));
            }
        };
        let result: Result<(), String> = async {
            let sftp = crate::sftp::get(&st.sftps, sftp_id)?;
            if direction == "upload" {
                let mut local = tokio::fs::File::open(&local_path)
                    .await
                    .map_err(|e| format!("打开本地文件失败: {e}"))?;
                let mut remote = sftp
                    .open_with_flags(&remote_path, OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE)
                    .await
                    .map_err(|e| format!("打开远端文件失败: {e}"))?;
                let mut buf = vec![0u8; 256 * 1024];
                let mut done: u64 = 0;
                loop {
                    if st.cancels.0.lock().get(&id).map(|c| *c.borrow()).unwrap_or(false) {
                        cancelled = true;
                        break;
                    }
                    let n = local.read(&mut buf).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    remote
                        .write_all(&buf[..n])
                        .await
                        .map_err(|e| format!("写入远端失败: {e}"))?;
                    done += n as u64;
                    emit_throttled(done, total, &app2, id);
                }
                let _ = remote.flush().await;
            } else {
                let mut remote = sftp
                    .open(&remote_path)
                    .await
                    .map_err(|e| format!("打开远端文件失败: {e}"))?;
                let mut local = tokio::fs::File::create(&local_path)
                    .await
                    .map_err(|e| format!("创建本地文件失败: {e}"))?;
                let mut buf = vec![0u8; 256 * 1024];
                let mut done: u64 = 0;
                loop {
                    if st.cancels.0.lock().get(&id).map(|c| *c.borrow()).unwrap_or(false) {
                        cancelled = true;
                        break;
                    }
                    let n = remote.read(&mut buf).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    local.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
                    done += n as u64;
                    emit_throttled(done, total, &app2, id);
                }
                let _ = local.flush().await;
            }
            Ok(())
        }
        .await;

        st.cancels.0.lock().remove(&id);
        if cancelled {
            set_status(&st.tasks, id, &app2, "cancelled", json!({}));
        } else if let Err(e) = result {
            set_status(&st.tasks, id, &app2, "failed", json!({ "error": e }));
        } else {
            set_status(&st.tasks, id, &app2, "done", json!({}));
        }
    });

    Ok(json!({ "ok": true, "id": id, "status": "queued", "received": 0, "total": total }))
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
            start(state.clone(), app.clone(), "download".into(), sftp_id, lpath.to_string_lossy().to_string(), rpath).await?;
            count += 1;
        }
    }
    Ok(count)
}
