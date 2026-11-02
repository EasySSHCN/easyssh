/* EasySSH v4：SFTP 远端文件操作（对应 3.x sftp:* 命令）
 * 每个会话可打开一个 SFTP 通道（子命令按需开启，关闭窗口时随会话释放）。
 * 传输队列见 xfer.rs。 */
use std::collections::HashMap;


use std::sync::Arc;

use parking_lot::Mutex;
use russh_sftp::client::SftpSession;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use crate::ssh::SessionMap;

#[derive(Clone, Default)]
pub struct SftpMap(pub std::sync::Arc<Mutex<HashMap<u64, SftpEntry>>>);
impl SftpMap {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(Mutex::new(HashMap::new())))
    }
}

/// v4.3.0 健壮性修复:通道记录归属会话——旧实现 open() 复用 Map 中任意通道,
/// 多标签并发 SFTP 会串到其他会话的服务器上(正确性 bug)
#[derive(Clone)]
pub struct SftpEntry {
    pub session_id: u64,
    pub sftp: Arc<SftpSession>,
}

static NEXT_SFTP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 为会话打开（或复用**本会话自己的**）SFTP 通道，返回 sftpId
pub async fn open(sessions: &SessionMap, sftps: &SftpMap, session_id: u64) -> Result<u64, String> {
    // 仅复用本会话的通道
    {
        let m = sftps.0.lock();
        for (k, e) in m.iter() {
            if e.session_id == session_id {
                return Ok(*k);
            }
        }
    }
    let sess = sessions
        .0
        .lock()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| "会话不存在".to_string())?;
    let mut chan = sess
        .handle
        .lock()
        .await
        .channel_open_session()
        .await
        .map_err(|e| format!("打开通道失败: {e}"))?;
    chan.request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("请求 sftp 子系统失败: {e}"))?;
    let sftp = SftpSession::new(chan.into_stream())
        .await
        .map_err(|e| format!("SFTP 初始化失败: {e}"))?;
    let id = NEXT_SFTP_ID.fetch_add(1, Ordering::SeqCst);
    sftps.0.lock().insert(id, SftpEntry { session_id, sftp: Arc::new(sftp) });
    Ok(id)
}

pub fn get(sftps: &SftpMap, id: u64) -> Result<Arc<SftpSession>, String> {
    sftps
        .0
        .lock()
        .get(&id)
        .map(|e| e.sftp.clone())
        .ok_or_else(|| "SFTP 未打开".to_string())
}

/// v4.3.0:会话断开时释放其 SFTP 通道(渲染端缓存失效联动)
pub fn remove_session(sftps: &SftpMap, session_id: u64) {
    sftps.0.lock().retain(|_, e| e.session_id != session_id);
}

/// v4.3.0:SFTP 通道自愈——按 sftpId 移除失效通道,下次 open 重新建立
pub fn remove_sftp(sftps: &SftpMap, sftp_id: u64) {
    sftps.0.lock().remove(&sftp_id);
}

/// 列目录：[{ name, isDir, size, mtime }]
pub async fn list(sftps: &SftpMap, sftp_id: u64, dir: &str) -> Result<Vec<serde_json::Value>, String> {
    let sftp = get(sftps, sftp_id)?;
    let entries = sftp.read_dir(dir).await.map_err(|e| format!("读取目录失败: {e}"))?;
    let mut out = Vec::new();
    for e in entries {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let md = e.metadata();
        out.push(json!({
            "name": name,
            "type": if md.is_dir() { "dir" } else { "file" },
            "isDir": md.is_dir(),
            "size": md.size.unwrap_or(0),
            "mtime": md.mtime.unwrap_or(0),
        }));
    }
    Ok(out)
}

pub async fn metadata(sftps: &SftpMap, sftp_id: u64, path: &str) -> Result<serde_json::Value, String> {
    let sftp = get(sftps, sftp_id)?;
    let md = sftp.metadata(path).await.map_err(|e| format!("读取信息失败: {e}"))?;
    Ok(json!({ "isDir": md.is_dir(), "size": md.size.unwrap_or(0), "mtime": md.mtime.unwrap_or(0) }))
}

/// v4.3.2 新增：递归计算目录占用（显式栈遍历，防递归失控）。
/// 防护：深度 > max_depth(40) 停止下钻、总条目 > max_entries(20 万) 截断、
/// 无权限子目录跳过并计数（skipped）；truncated=true 表示结果为下限值。
pub async fn dir_size(sftps: &SftpMap, sftp_id: u64, path: &str) -> Result<serde_json::Value, String> {
    let sftp = get(sftps, sftp_id)?;
    const MAX_DEPTH: u32 = 40;
    const MAX_ENTRIES: u64 = 200_000;
    let mut stack = vec![(path.to_string(), 0u32)];
    let mut size: u64 = 0;
    let mut files: u64 = 0;
    let mut dirs: u64 = 0;
    let mut skipped: u64 = 0;
    let mut visited: u64 = 0;
    let mut truncated = false;
    while let Some((p, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_ENTRIES {
            truncated = true;
            break;
        }
        let entries = match sftp.read_dir(&p).await {
            Ok(e) => e,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        dirs += 1;
        if depth >= MAX_DEPTH {
            truncated = true;
            continue;
        }
        for e in entries {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let md = e.metadata();
            let child = format!("{}/{}", p.trim_end_matches('/'), name);
            if md.is_dir() {
                stack.push((child, depth + 1));
            } else {
                size += md.size.unwrap_or(0);
                files += 1;
            }
        }
    }
    Ok(json!({
        "ok": true, "path": path, "size": size, "files": files,
        "dirs": dirs, "skipped": skipped, "truncated": truncated,
    }))
}

pub async fn rename(sftps: &SftpMap, sftp_id: u64, old_path: &str, new_path: &str) -> Result<Value, String> {
    let sftp = get(sftps, sftp_id)?;
    sftp.rename(old_path, new_path)
        .await
        .map_err(|e| format!("重命名失败: {e}"))?;
    Ok(json!({ "ok": true }))
}

pub async fn remove_file(sftps: &SftpMap, sftp_id: u64, path: &str) -> Result<Value, String> {
    let sftp = get(sftps, sftp_id)?;
    sftp.remove_file(path).await.map_err(|e| format!("删除失败: {e}"))?;
    Ok(json!({ "ok": true }))
}

pub async fn remove_dir(sftps: &SftpMap, sftp_id: u64, path: &str) -> Result<Value, String> {
    let sftp = get(sftps, sftp_id)?;
    sftp.remove_dir(path).await.map_err(|e| format!("删除目录失败: {e}"))?;
    Ok(json!({ "ok": true }))
}

/// v4.2.3 修复:统一删除入口——文件直接删,目录深度优先递归删除(对齐 3.x sftp:remove 语义)。
/// 先尝试按文件删;失败则按目录读列表递归删内容后 rmdir。
async fn remove_recursive(sftp: &SftpSession, path: &str) -> Result<(), String> {
    if sftp.remove_file(path).await.is_ok() {
        return Ok(());
    }
    let entries = sftp
        .read_dir(path)
        .await
        .map_err(|e| format!("删除失败: {e}"))?;
    for e in entries {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let child = format!("{}/{}", path.trim_end_matches('/'), name);
        Box::pin(remove_recursive(sftp, &child)).await?;
    }
    sftp.remove_dir(path).await.map_err(|e| format!("删除目录失败: {e}"))?;
    Ok(())
}

pub async fn remove_path(sftps: &SftpMap, sftp_id: u64, path: &str) -> Result<Value, String> {
    let sftp = get(sftps, sftp_id)?;
    remove_recursive(&sftp, path).await?;
    Ok(json!({ "ok": true }))
}

pub async fn mkdir(sftps: &SftpMap, sftp_id: u64, path: &str) -> Result<Value, String> {
    let sftp = get(sftps, sftp_id)?;
    sftp.create_dir(path).await.map_err(|e| format!("新建目录失败: {e}"))?;
    Ok(json!({ "ok": true }))
}

/// 规范化路径（返回绝对路径）
pub async fn canonpath(sftps: &SftpMap, sftp_id: u64) -> Result<String, String> {
    let sftp = get(sftps, sftp_id)?;
    sftp.canonicalize(".").await.map_err(|e| format!("获取路径失败: {e}"))
}

use std::sync::atomic::Ordering;
