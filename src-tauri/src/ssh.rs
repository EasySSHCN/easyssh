/* EasySSH v4 PoC：russh SSH 客户端会话管理
 * 每个 SSH 会话：认证后打开 PTY shell Channel，由一个后台任务独占该 Channel——
 *   输出: ChannelMsg::Data → tauri event("ssh-data")
 *   输入: command ssh_write → mpsc → 该任务 channel.data() 写入
 *   断开: command ssh_disconnect → watch 信号 → channel.close()
 * PoC 阶段不做服务器指纹校验（正式版按 3.x known_hosts 协议核对）。 */
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use russh::client::{self, Handle};
use russh::{Channel, ChannelMsg};
use russh::Disconnect;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct SessionHandle {
    pub id: u64,
    pub input_tx: mpsc::Sender<Vec<u8>>,
    pub resize_tx: mpsc::Sender<(u32, u32)>,
    pub close_tx: tokio::sync::watch::Sender<bool>,
}

pub struct SessionMap(pub Mutex<HashMap<u64, SessionHandle>>);
impl SessionMap {
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct ClientHandler;

#[async_trait::async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true) // PoC: 信任所有主机密钥
    }
}

pub async fn connect(
    app: AppHandle,
    sessions: &SessionMap,
    host: String,
    port: u16,
    user: String,
    password: String,
    cols: u32,
    rows: u32,
) -> Result<u64, String> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        ..Default::default()
    });

    let mut handle: Handle<ClientHandler> = client::connect(config, (host.as_str(), port), ClientHandler)
        .await
        .map_err(|e| format!("SSH 连接失败: {e}"))?;

    let authed = handle
        .authenticate_password(&user, &password)
        .await
        .map_err(|e| format!("认证失败: {e}"))?;
    if !authed {
        let _ = handle.disconnect(Disconnect::ByApplication, "auth failed", "en").await;
        return Err("服务器拒绝了用户名/密码".into());
    }

    let mut channel: Channel<client::Msg> = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("打开会话通道失败: {e}"))?;
    channel
        .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(|e| format!("申请 PTY 失败: {e}"))?;
    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("启动 shell 失败: {e}"))?;

    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(256);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(16);
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(false);

    let sess = SessionHandle { id, input_tx, resize_tx, close_tx };
    sessions.0.lock().insert(id, sess);

    // I/O 任务：独占 Channel，select 输入/尺寸/关闭/输出
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = close_rx.changed() => {
                    if *close_rx.borrow() { break; }
                }
                Some(data) = input_rx.recv() => {
                    if channel.data(&data[..]).await.is_err() { break; }
                }
                Some((cols, rows)) = resize_rx.recv() => {
                    let _ = channel.window_change(cols, rows, 0, 0).await;
                }
                msg = channel.wait() => {
                    match msg {
                        Some(ChannelMsg::Data { ref data }) => {
                            let _ = app.emit("ssh-data", serde_json::json!({ "id": id, "data": String::from_utf8_lossy(data) }));
                        }
                        Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                            let _ = app.emit("ssh-data", serde_json::json!({ "id": id, "data": String::from_utf8_lossy(data) }));
                        }
                        Some(_) => {}
                        None => break, // 远端关闭
                    }
                }
                else => break,
            }
        }
        let _ = app.emit("ssh-closed", serde_json::json!({ "id": id }));
    });

    Ok(id)
}

pub async fn write(sessions: &SessionMap, id: u64, data: &str) -> Result<(), String> {
    let sess = sessions
        .0
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| "会话不存在".to_string())?;
    sess.input_tx
        .send(data.as_bytes().to_vec())
        .await
        .map_err(|_| "会话已关闭".to_string())
}

pub async fn resize(sessions: &SessionMap, id: u64, cols: u32, rows: u32) -> Result<(), String> {
    let sess = sessions
        .0
        .lock()
        .get(&id)
        .cloned()
        .ok_or_else(|| "会话不存在".to_string())?;
    sess.resize_tx
        .send((cols, rows))
        .await
        .map_err(|_| "会话已关闭".to_string())
}

pub async fn disconnect(sessions: &SessionMap, id: u64) -> Result<(), String> {
    let sess = sessions
        .0
        .lock()
        .remove(&id)
        .ok_or_else(|| "会话不存在".to_string())?;
    let _ = sess.close_tx.send(true);
    Ok(())
}
