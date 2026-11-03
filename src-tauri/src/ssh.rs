/* EasySSH v4：russh SSH 客户端会话管理（对应 3.x main.js ssh:connect）
 * - 主机指纹：SHA256:<b64(wire pubkey 去padding)>，与 3.x ssh2 hostVerifier 一致
 *   · 渲染端传 expectedFingerprint 时严格比对，不匹配拒绝连接（MITM 防护）
 *   · 首次连接（无期望指纹）放行，实际指纹随连接结果返回，由渲染端确认后保存
 * - 认证：password / private key(+passphrase) / keyboard-interactive(回填密码)
 * - 会话：PTY shell；单任务独占 Channel，输入 mpsc / 尺寸 mpsc / 关闭 watch */
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use russh::client::{self, Handle};
use russh::{Channel, ChannelMsg, Disconnect};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct SessionHandle {
    pub id: u64,
    /// russh 连接句柄：保活连接 + 为 SFTP/exec/端口转发打开新通道
    pub handle: Arc<tokio::sync::Mutex<Handle<ClientHandler>>>,
    pub input_tx: mpsc::Sender<Vec<u8>>,
    pub resize_tx: mpsc::Sender<(u32, u32)>,
    pub close_tx: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone, Default)]
pub struct SessionMap(pub std::sync::Arc<Mutex<HashMap<u64, SessionHandle>>>);
impl SessionMap {
    pub fn new() -> Self {
        Self(std::sync::Arc::new(Mutex::new(HashMap::new())))
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// 指纹计算：与 3.x 一致 SHA256:<b64 去padding>（russh_keys::fingerprint 即此格式）
pub fn fingerprint_of(pubkey: &russh_keys::key::PublicKey) -> String {
    format!("SHA256:{}", pubkey.fingerprint())
}

#[derive(Clone)]
pub struct ClientHandler {
    expected_fp: Option<String>,
    actual_fp: Arc<Mutex<Option<String>>>,
    password: Arc<Mutex<Option<String>>>,
}

impl ClientHandler {
    /// v5(M2)：net.rs 跳板链连接复用（跳板不校验指纹——凭据复用已存会话）
    pub(crate) fn new(expected_fp: Option<String>, password: String) -> Self {
        Self {
            expected_fp,
            actual_fp: Arc::new(Mutex::new(None)),
            password: Arc::new(Mutex::new(Some(password))),
        }
    }
    pub(crate) fn actual_fingerprint(&self) -> Option<String> {
        self.actual_fp.lock().clone()
    }
}

#[async_trait::async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let fp = fingerprint_of(server_public_key);
        *self.actual_fp.lock() = Some(fp.clone());
        match &self.expected_fp {
            None => Ok(true), // 首次连接放行，确认流程在渲染端
            Some(want) => Ok(&fp == want),
        }
    }

    async fn data(
        &mut self,
        _channel: russh::ChannelId,
        _data: &[u8],
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 连接结果：id + 实际指纹（渲染端据此弹首次确认/不匹配弹窗）
pub struct ConnectOutcome {
    pub id: u64,
    pub fingerprint: Option<String>,
}

pub async fn connect(
    app: AppHandle,
    sessions: &SessionMap,
    config: &super::configstate::ConfigState,
    opts: &super::types::ConnectOpts,
) -> Result<ConnectOutcome, String> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    // v5(M2)/F-M2-05：先解析会话的代理/跳板配置建立底层流（无配置时为普通直连）
    let stream = super::net::open_stream(config, &opts.host, opts.port, &opts.user).await?;
    let handler = ClientHandler::new(opts.expected_fingerprint.clone(), opts.password.clone());
    let actual_fp_holder = handler.clone();
    let config_cfg = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(10)),
        inactivity_timeout: None,
        ..Default::default()
    });

    let mut handle: Handle<ClientHandler> = match client::connect_stream(config_cfg, stream, handler).await {
        Ok(h) => h,
        Err(e) => {
            let fp = actual_fp_holder.actual_fingerprint();
            // 指纹不匹配被 handler 拒绝 → 专用错误前缀，渲染端弹「更新指纹」确认框
            if let (Some(actual), Some(stored)) = (fp, &opts.expected_fingerprint) {
                if actual != *stored {
                    return Err(format!("FINGERPRINT_MISMATCH:{actual}:{stored}"));
                }
            }
            return Err(format!("SSH 连接失败: {e}"));
        }
    };

    let actual_fp_val = actual_fp_holder.actual_fingerprint();

    // 认证（v5(M2)：抽取为 authenticate 供跳板链复用）
    if let Err(e) = authenticate(
        &mut handle,
        &opts.user,
        &opts.password,
        opts.private_key.as_deref(),
        opts.passphrase.as_deref(),
    )
    .await
    {
        let _ = handle.disconnect(Disconnect::ByApplication, "auth failed", "en").await;
        return Err(e);
    }

    // v5(M2)/F-M2-05：认证逻辑抽取为 authenticate（本函数尾部定义），跳板链复用

    let mut channel: Channel<client::Msg> = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("打开会话通道失败: {e}"))?;
    channel
        .request_pty(false, "xterm-256color", opts.cols, opts.rows, 0, 0, &[])
        .await
        .map_err(|e| format!("申请 PTY 失败: {e}"))?;
    channel
        .request_shell(false)
        .await
        .map_err(|e| format!("启动 shell 失败: {e}"))?;

    let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(256);
    let (resize_tx, mut resize_rx) = mpsc::channel::<(u32, u32)>(16);
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(false);

    let sess = SessionHandle {
        id,
        handle: Arc::new(tokio::sync::Mutex::new(handle)),
        input_tx,
        resize_tx,
        close_tx,
    };
    sessions.0.lock().insert(id, sess);

    let app2 = app.clone();
    // v4.3.0 性能:输出批量合并——旧实现每个 SSH 数据分片单独 emit 一次 JSON 事件,
    // 高吞吐输出(cat 大文件/top)时 IPC 每秒数百次拖垮 WebView2。
    // 现在攒 10ms 或 32KB 再发一次,并保留不完整的 UTF-8 尾字节到下一批(避免多字节字符被截断成乱码)。
    const FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
    const FLUSH_THRESHOLD: usize = 32 * 1024;
    tokio::spawn(async move {
        let mut pending: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut flush_at: Option<tokio::time::Instant> = None;
        let flush = |pending: &mut Vec<u8>, app: &AppHandle| {
            if pending.is_empty() {
                return;
            }
            // 找到最长的完整 UTF-8 前缀,尾部不完整字节留到下一批
            let valid = match std::str::from_utf8(pending) {
                Ok(_) => pending.len(),
                Err(e) => e.valid_up_to(),
            };
            let data = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            if !data.is_empty() {
                let _ = app.emit("ssh-data", serde_json::json!({ "id": id, "data": data }));
            }
        };
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
                        Some(ChannelMsg::Data { ref data }) | Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                            if pending.is_empty() {
                                flush_at = Some(tokio::time::Instant::now() + FLUSH_INTERVAL);
                            }
                            pending.extend_from_slice(data);
                            if pending.len() >= FLUSH_THRESHOLD {
                                flush(&mut pending, &app2);
                                flush_at = None;
                            }
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                _ = async {
                    match flush_at {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    flush(&mut pending, &app2);
                    flush_at = None;
                }
                else => break,
            }
        }
        flush(&mut pending, &app2); // 断开前冲刷残留
        let _ = app2.emit("ssh-closed", serde_json::json!({ "id": id }));
    });

    Ok(ConnectOutcome { id, fingerprint: actual_fp_val })
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
    let _ = sess
        .handle
        .lock()
        .await
        .disconnect(Disconnect::ByApplication, "user disconnect", "en")
        .await;
    Ok(())
}

/// v5(M2)/F-M2-05：认证流程（私钥 → 密码 → keyboard-interactive 回填密码），
/// 目标机与跳板机共用。失败时返回描述性错误（调用方负责 disconnect）。
pub(crate) async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    password: &str,
    private_key: Option<&str>,
    passphrase: Option<&str>,
) -> Result<(), String> {
    let mut authed = false;
    let mut auth_err = String::new();
    let use_key = private_key.map(|p| !p.trim().is_empty()).unwrap_or(false);
    if use_key {
        let key_path = private_key.unwrap_or("").trim().to_string();
        let pass = passphrase.filter(|p| !p.is_empty());
        match russh_keys::load_secret_key(&key_path, pass) {
            Ok(key) => match handle.authenticate_publickey(user, Arc::new(key)).await {
                Ok(a) if a => authed = true,
                Ok(_) => auth_err = "服务器拒绝了私钥认证".into(),
                Err(e) => auth_err = format!("私钥认证失败: {e}"),
            },
            Err(e) => auth_err = format!("无法读取私钥: {e}"),
        }
        if !authed && auth_err.is_empty() {
            auth_err = "私钥认证被拒绝".into();
        }
    }
    if !authed && !use_key {
        match handle.authenticate_password(user, password).await {
            Ok(a) if a => authed = true,
            Ok(_) => auth_err = "服务器拒绝了用户名/密码".into(),
            Err(e) => auth_err = format!("认证失败: {e}"),
        }
        if !authed {
            // keyboard-interactive：start → 收到 info request 时回填密码（对应 3.x tryKeyboard）
            match handle.authenticate_keyboard_interactive_start(user, None).await {
                Ok(mut resp) => loop {
                    resp = match resp {
                        russh::client::KeyboardInteractiveAuthResponse::Success => {
                            authed = true;
                            break;
                        }
                        russh::client::KeyboardInteractiveAuthResponse::Failure => break,
                        russh::client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                            let pw = password.to_string();
                            let answers: Vec<String> = prompts.iter().map(|_| pw.clone()).collect();
                            match handle.authenticate_keyboard_interactive_respond(answers).await {
                                Ok(r) => r,
                                Err(_) => break,
                            }
                        }
                    };
                },
                Err(_) => {}
            }
        }
    }
    if authed {
        Ok(())
    } else {
        Err(auth_err)
    }
}
