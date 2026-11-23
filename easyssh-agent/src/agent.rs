//! Agent 主循环：WS 握手、心跳（PING/PONG 双向应答）、DIAL→本地 TCP、
//! DATA/CLOSE 双向字节转发、断线指数退避重连（方案第 3/5 节）。
//!
//! 可测性分层：
//! - [`agent_core`]：纯帧级逻辑，入站/出站均为 `mpsc::Envelope` 通道，
//!   单测用内存假网关 + 回环 TcpListener 覆盖，不需要网络；
//! - [`serve_frames`]：WS Binary ↔ Envelope 通道的粘合层；
//! - [`run_once`] / [`run_agent`]：连接级循环与退避重连。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, sleep, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderMap as WsHeaderMap, HeaderValue};
use tokio_tungstenite::{tungstenite, Connector, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;

use crate::backoff::Backoff;
use crate::config::AgentConfig;
use crate::wire::{cut_envelope, parse_dial_body, Envelope, Op};

/// 帧级会话通道缓冲（与网关侧 AgentSlot 通道容量同量级）。
const CHANNEL_BUF: usize = 64;

/// agent_core 运行参数（从 AgentConfig 摘取帧级所需字段，便于测试注入）。
#[derive(Debug, Clone)]
pub struct CoreCtx {
    /// 唯一允许 DIAL 的本地后端。
    pub local_backend: String,
    /// 本地 TCP connect 超时。
    pub dial_timeout: Duration,
    /// 单连接并发 session 上限。
    pub max_sessions: usize,
    /// 心跳间隔（本端主动发 PING(0) 保活 NAT；入站 PING 一律回 PONG）。
    pub heartbeat: Duration,
    /// 入站静默判死窗口（heartbeat × 3，方案口径）。
    pub silence: Duration,
}

impl CoreCtx {
    pub fn from_config(cfg: &AgentConfig) -> Self {
        let hb = Duration::from_secs(cfg.heartbeat_secs);
        Self {
            local_backend: cfg.local_backend.clone(),
            dial_timeout: Duration::from_secs(cfg.dial_timeout_secs),
            max_sessions: cfg.max_sessions,
            silence: hb * 3,
            heartbeat: hb,
        }
    }
}

/// 入站静默判死（含心跳超时）：超过 `silence` 未收到任何报文即断线重连。
fn is_silent(last_seen: std::time::Instant, silence: Duration) -> bool {
    last_seen.elapsed() >= silence
}

/// Agent 帧级核心（与网关侧 route_from_agent 语义对称，独立实现）：
/// - PING → 回 PONG（同 session）；
/// - PONG → 心跳应答（无副作用）；
/// - DIAL → 校验 backend ∈ {配置内 local_backend} → 本地 TCP connect
///   （超时即 OPEN_ERR）→ OPEN_OK 后双向转发；
/// - DATA/CLOSE → 投递到对应 session 的转发通道（CLOSE 后摘除）；
/// - OPEN_OK/OPEN_ERR 上行（GW→Agent 方向不应出现）→ 忽略。
/// 静默超 `ctx.silence` 或入站通道关闭即返回（连接判死）。
pub async fn agent_core(
    mut in_rx: mpsc::Receiver<Envelope>,
    out_tx: mpsc::Sender<Envelope>,
    ctx: CoreCtx,
) {
    let sessions: Arc<Mutex<HashMap<u64, mpsc::Sender<Envelope>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut hb = interval(ctx.heartbeat);
    hb.tick().await; // 跳过 interval 立即触发的首个 tick
    let mut last_seen = std::time::Instant::now();

    loop {
        tokio::select! {
            env = in_rx.recv() => {
                let Some(env) = env else { break };
                last_seen = std::time::Instant::now();
                let (op, session) = (env.op, env.session);
                match op {
                    Op::Ping => {
                        if out_tx.send(Envelope::new(Op::Pong, session, Vec::new())).await.is_err() {
                            break;
                        }
                    }
                    Op::Pong => {}
                    Op::Dial => {
                        if open_local_backend(&env, &out_tx, &sessions, &ctx).await.is_err() {
                            break;
                        }
                    }
                    Op::Data | Op::Close => {
                        let mut map = sessions.lock().await;
                        if let Some(tx) = map.get(&session) {
                            let ok = tx.send(env).await.is_ok();
                            if op == Op::Close || !ok {
                                map.remove(&session);
                            }
                        }
                        // 未注册 session 的迟到帧直接丢弃（连接已被清理）
                    }
                    Op::OpenOk | Op::OpenErr => {}
                }
            }
            _ = hb.tick() => {
                // 主动心跳：PING(session=0)；发不出去 = 连接已死。
                if out_tx.send(Envelope::new(Op::Ping, 0, Vec::new())).await.is_err() {
                    break;
                }
                if is_silent(last_seen, ctx.silence) {
                    tracing::warn!("agent 入站静默超过判死窗口，断开重连");
                    break;
                }
            }
        }
    }

    // 连接收尾：关闭全部 session（各 pump 任务随通道关闭自然退出）。
    sessions.lock().await.clear();
}

/// 处理一条 DIAL：校验 → connect → OPEN_OK/OPEN_ERR → 双向 pump。
/// 返回 Err 表示出站通道已死（连接级失败）。
async fn open_local_backend(
    dial: &Envelope,
    out_tx: &mpsc::Sender<Envelope>,
    sessions: &Arc<Mutex<HashMap<u64, mpsc::Sender<Envelope>>>>,
    ctx: &CoreCtx,
) -> Result<(), ()> {
    let reply = |op: Op, msg: &[u8]| {
        let msg = msg.to_vec();
        let session = dial.session;
        let out_tx = out_tx.clone();
        async move {
            let _ = out_tx.send(Envelope::new(op, session, msg)).await;
        }
    };

    // 目标白名单：仅配置内 local_backend（方案硬约束：不接受任意目标）。
    let backend = match parse_dial_body(&dial.body) {
        Some(b) if b == ctx.local_backend => b,
        _ => {
            reply(Op::OpenErr, b"backend not allowed").await;
            return Ok(());
        }
    };

    // 并发上限（与网关 max_sessions_per_conn 对齐）。
    // 同 session 重 DIAL：摘除旧条目（覆盖放行，语义同网关 bind_session）；
    // 旧 pump 任务随下行通道关闭自然退出。
    let mut map = sessions.lock().await;
    map.remove(&dial.session);
    if map.len() >= ctx.max_sessions {
        reply(Op::OpenErr, b"session cap exceeded").await;
        return Ok(());
    }

    let tcp = match timeout(ctx.dial_timeout, TcpStream::connect(&backend)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let msg = format!("connect {backend}: {e}");
            tracing::warn!(session = dial.session, %backend, "DIAL 失败: {e}");
            reply(Op::OpenErr, msg.as_bytes()).await;
            return Ok(());
        }
        Err(_) => {
            let msg = format!("connect {backend}: timeout");
            tracing::warn!(session = dial.session, %backend, "DIAL 超时");
            reply(Op::OpenErr, msg.as_bytes()).await;
            return Ok(());
        }
    };

    let (fwd_tx, fwd_rx) = mpsc::channel::<Envelope>(CHANNEL_BUF);
    map.insert(dial.session, fwd_tx);
    drop(map);

    spawn_session_pump(dial.session, tcp, fwd_rx, Arc::clone(sessions), out_tx.clone());
    let _ = out_tx
        .send(Envelope::new(Op::OpenOk, dial.session, Vec::new()))
        .await;
    Ok(())
}

/// 启动一个 session 的双向转发任务：
/// - tcp → DATA（上行）→ EOF 发 CLOSE 并摘除；
/// - fwd_rx（下行 DATA/CLOSE）→ tcp 写；CLOSE 或通道关闭即停止。
fn spawn_session_pump(
    session: u64,
    tcp: TcpStream,
    mut fwd_rx: mpsc::Receiver<Envelope>,
    sessions: Arc<Mutex<HashMap<u64, mpsc::Sender<Envelope>>>>,
    out_tx: mpsc::Sender<Envelope>,
) {
    let (mut rd, mut wr) = tokio::io::split(tcp);
    tokio::spawn(async move {
        let mut buf = [0u8; 16 * 1024];
        let read_done = loop {
            tokio::select! {
                n = rd.read(&mut buf) => {
                    match n {
                        Ok(0) | Err(_) => break true,
                        Ok(n) => {
                            if out_tx.send(Envelope::new(Op::Data, session, buf[..n].to_vec())).await.is_err() {
                                break true;
                            }
                        }
                    }
                }
                env = fwd_rx.recv() => {
                    match env {
                        Some(Envelope { op: Op::Data, body, .. }) => {
                            if wr.write_all(&body).await.is_err() { break false; }
                        }
                        Some(Envelope { op: Op::Close, .. }) | None => break false,
                        _ => {}
                    }
                }
            }
        };
        // 单侧结束后关闭整条 session（TCP 半关闭对 ssh 无意义）。
        let _ = wr.shutdown().await;
        sessions.lock().await.remove(&session);
        if read_done {
            // 上行 EOF：通知网关该 session 已结束。
            let _ = out_tx.send(Envelope::new(Op::Close, session, Vec::new())).await;
        }
    });
}

/// WS 连接帧级粘合层：Binary ↔ Envelope 通道。
/// 任一泵结束（连接关闭/解码失败/出站通道断）即返回。
pub async fn serve_frames(
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    in_tx: mpsc::Sender<Envelope>,
    mut out_rx: mpsc::Receiver<Envelope>,
) {
    let (mut sink, mut stream) = socket.split();

    // 出站泵：Envelope → WS Binary。
    let mut out_task = tokio::spawn(async move {
        while let Some(env) = out_rx.recv().await {
            if sink.send(Message::Binary(env.to_bytes())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // 入站泵：WS Binary → 跨帧缓冲解码 → in_tx。
    let mut in_task = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Binary(chunk) => {
                    buf.extend_from_slice(&chunk);
                    loop {
                        match cut_envelope(&buf) {
                            Ok(Some((env, used))) => {
                                buf.drain(..used);
                                if in_tx.send(env).await.is_err() {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(fault) => {
                                tracing::warn!(?fault, "agent wire fault, dropping connection");
                                return;
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {} // Text/Ping/Pong 控制帧：协议层自动应答，忽略
            }
        }
    });

    // 任一泵退出即视为连接死亡。
    tokio::select! {
        _ = &mut out_task => {}
        _ = &mut in_task => {}
    }    out_task.abort();
    in_task.abort();
}

/// 建立一次 /agent/connect 连接并跑完它的生命周期。
/// 返回 Ok 表示对端正常关闭（走重连），Err 为握手或协议失败（走重连）。
pub async fn run_once(cfg: &AgentConfig, token: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut request = cfg.ws_url().into_client_request()?;
    let headers: &mut WsHeaderMap = request.headers_mut();
    headers.insert(
        tungstenite::http::header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {token}"))?,
    );

    let connector = build_connector(cfg.accept_invalid_certs, cfg.ws_url().starts_with("wss://"))?;
    let (socket, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector).await?;
    tracing::info!(resource_id = %cfg.resource_id, "agent connected to gateway");

    let (in_tx, in_rx) = mpsc::channel::<Envelope>(CHANNEL_BUF);
    let (out_tx, out_rx) = mpsc::channel::<Envelope>(CHANNEL_BUF);
    let ctx = CoreCtx::from_config(cfg);

    // 帧级核心与 WS 泵并行；核心退出（静默判死）即断开整条连接。
    let core = tokio::spawn(agent_core(in_rx, out_tx.clone(), ctx));
    let frames = serve_frames(socket, in_tx, out_rx);
    tokio::pin!(core);
    tokio::select! {
        _ = frames => {}
        _ = &mut core => {}
    }
    core.abort();
    Ok(())
}

/// 顶层重连循环（方案：断线指数退避，封顶 30s）：
/// - 握手失败 → 按退避序列等待后重试；
/// - 连接建立成功后发生的断线 → 退避复位，从 base 重新开始；
/// - `shutdown` 信号触发后立即退出。
pub async fn run_agent(
    cfg: AgentConfig,
    token: String,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut backoff = Backoff::new(cfg.backoff_base_ms, cfg.backoff_cap_ms);
    loop {
        let connected = match run_once(&cfg, &token).await {
            Ok(()) => {
                tracing::info!(resource_id = %cfg.resource_id, "与网关的连接结束，准备重连");
                true
            }
            Err(e) => {
                tracing::warn!("连接失败: {e}");
                false
            }
        };
        if connected {
            // 曾成功建立连接：本轮断线不算连续失败，退避复位。
            backoff.reset();
        }
        let wait = backoff.next();
        tracing::info!(wait_secs = wait.as_secs_f32(), "退避后重连");
        tokio::select! {
            _ = sleep(wait) => {}
            _ = shutdown.changed() => {
                tracing::info!("收到停机信号，agent 退出");
                return;
            }
        }
    }
}

/// TLS 连接器：ws:// 走明文；wss:// 默认走 webpki 根证书，
/// `accept_invalid_certs=true` 时换成"放行任何服务端证书"的危险校验器
/// （自签网关首版联调用）。
fn build_connector(
    accept_invalid_certs: bool,
    is_wss: bool,
) -> Result<Option<Connector>, Box<dyn std::error::Error + Send + Sync>> {
    if !is_wss {
        return Ok(Some(Connector::Plain));
    }
    if !accept_invalid_certs {
        // 内建 webpki-roots 校验（feature rustls-tls-webpki-roots）。
        return Ok(None);
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    // supported_verify_schemes 用标准算法清单；verify_tls12/13 直接委托 ring。
    let schemes: Vec<rustls::SignatureScheme> = provider
        .signature_verification_algorithms
        .supported_schemes()
        .iter()
        .copied()
        .collect();
    let verifier = Arc::new(danger::AcceptAnyServerCert { schemes });
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(Some(Connector::Rustls(Arc::new(tls))))
}

/// 自签联调用的危险校验器：仅跳过证书链验证（仍完成 TLS 握手与加密）。
mod danger {
    use rustls::client::danger::{ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub struct AcceptAnyServerCert {
        pub schemes: Vec<SignatureScheme>,
    }

    impl ServerCertVerifier for AcceptAnyServerCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
            verify_tls12_signature(
                message,
                cert,
                dss,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
            verify_tls13_signature(
                message,
                cert,
                dss,
                &rustls::crypto::ring::default_provider().signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.schemes.clone()
        }
    }
}

// ---------------------------------------------------------------------
// 单元测试：DIAL 校验/上限/回环转发（假网关内存回环 + 回环 TcpListener）
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 起一个 core，返回 (入站发送端, 出站接收端)。
    fn spawn_core(ctx: CoreCtx) -> (mpsc::Sender<Envelope>, mpsc::Receiver<Envelope>) {
        let (in_tx, in_rx) = mpsc::channel::<Envelope>(16);
        let (out_tx, out_rx) = mpsc::channel::<Envelope>(16);
        tokio::spawn(agent_core(in_rx, out_tx, ctx));
        (in_tx, out_rx)
    }

    fn test_ctx(backend: &str, cap: usize) -> CoreCtx {
        CoreCtx {
            local_backend: backend.to_string(),
            dial_timeout: Duration::from_secs(2),
            max_sessions: cap,
            heartbeat: Duration::from_secs(60),
            silence: Duration::from_secs(180),
        }
    }

    /// 回环 echo 服务：读到什么回什么，返回 (addr, shutdown 触发后关闭)。
    async fn spawn_echo_server() -> (std::net::SocketAddr, tokio::sync::oneshot::Sender<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let mut stop_rx = Box::pin(stop_rx);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    accepted = listener.accept() => {
                        let Ok((mut sock, _)) = accepted else { break };
                        tokio::spawn(async move {
                            let mut buf = [0u8; 4096];
                            loop {
                                match sock.read(&mut buf).await {
                                    Ok(0) | Err(_) => break,
                                    Ok(n) => if sock.write_all(&buf[..n]).await.is_err() { break },
                                }
                            }
                        });
                    }
                }
            }
        });
        (addr, stop_tx)
    }

    async fn recv_op(out_rx: &mut mpsc::Receiver<Envelope>, want: Op) -> Envelope {
        let env = out_rx.recv().await.expect("outbound frame");
        assert_eq!(
            env.op,
            want,
            "期望 {want:?}，实际 {:?}（session={}，body={:?}）",
            env.op,
            env.session,
            String::from_utf8_lossy(&env.body)
        );
        env
    }

    #[tokio::test]
    async fn ping_is_answered_with_pong_same_session() {
        let (in_tx, mut out_rx) = spawn_core(test_ctx("127.0.0.1:22", 8));
        in_tx.send(Envelope::new(Op::Ping, 42, Vec::new())).await.unwrap();
        let pong = recv_op(&mut out_rx, Op::Pong).await;
        assert_eq!(pong.session, 42);
        assert!(pong.body.is_empty());
    }

    #[tokio::test]
    async fn dial_wrong_backend_is_open_err() {
        let (in_tx, mut out_rx) = spawn_core(test_ctx("127.0.0.1:22", 8));
        // 非白名单目标（包括外网地址）一律 OPEN_ERR
        for backend in ["127.0.0.1:3389", "10.0.0.5:22", "evil.example.com:22"] {
            in_tx
                .send(Envelope::new(Op::Dial, 1, crate::wire::dial_body(backend)))
                .await
                .unwrap();
            let err = recv_op(&mut out_rx, Op::OpenErr).await;
            assert_eq!(err.session, 1);
        }
        // 非法 payload 同样 OPEN_ERR
        in_tx.send(Envelope::new(Op::Dial, 2, b"garbage".to_vec())).await.unwrap();
        recv_op(&mut out_rx, Op::OpenErr).await;
    }

    #[tokio::test]
    async fn dial_unreachable_backend_is_open_err_with_reason() {
        // 占用后立刻释放的端口（大概率无监听）
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = listener.local_addr().unwrap().port();
        drop(listener);
        let (in_tx, mut out_rx) = spawn_core(test_ctx(&format!("127.0.0.1:{dead_port}"), 8));
        in_tx
            .send(Envelope::new(Op::Dial, 9, crate::wire::dial_body(&format!("127.0.0.1:{dead_port}"))))
            .await
            .unwrap();
        let err = recv_op(&mut out_rx, Op::OpenErr).await;
        assert_eq!(err.session, 9);
        assert!(!err.body.is_empty(), "OPEN_ERR 需带原因");
    }

    #[tokio::test]
    async fn dial_then_bidirectional_echo_roundtrip() {
        let (echo_addr, stop_echo) = spawn_echo_server().await;
        let backend = echo_addr.to_string();
        let (in_tx, mut out_rx) = spawn_core(test_ctx(&backend, 8));

        in_tx
            .send(Envelope::new(Op::Dial, 7, crate::wire::dial_body(&backend)))
            .await
            .unwrap();
        recv_op(&mut out_rx, Op::OpenOk).await;

        // 客户端 → sshd 方向：DATA 注入，echo 原样返回
        in_tx.send(Envelope::new(Op::Data, 7, b"SSH-2.0-hello".to_vec())).await.unwrap();
        let mut got = Vec::new();
        while got.len() < b"SSH-2.0-hello".len() {
            let env = recv_op(&mut out_rx, Op::Data).await;
            assert_eq!(env.session, 7);
            got.extend_from_slice(&env.body);
        }
        assert_eq!(got, b"SSH-2.0-hello");

        // 关闭 session：下行 CLOSE → core 摘除（不回发 CLOSE；TCP EOF 才上行 CLOSE）
        in_tx.send(Envelope::new(Op::Close, 7, Vec::new())).await.unwrap();
        // 同 session 重新 DIAL 应成功（旧 session 已摘除）
        in_tx.send(Envelope::new(Op::Dial, 7, crate::wire::dial_body(&backend))).await.unwrap();
        recv_op(&mut out_rx, Op::OpenOk).await;
        in_tx.send(Envelope::new(Op::Close, 7, Vec::new())).await.unwrap();

        stop_echo.send(()).ok();
    }

    #[tokio::test]
    async fn server_side_eof_sends_close_upstream() {
        // 立即关闭的 listener 连接：connect 成功但读端马上 EOF
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // 接受后立即丢弃 socket（关闭）
            while let Ok((_s, _)) = listener.accept().await {
                drop(_s);
            }
        });
        let backend = addr.to_string();
        let (in_tx, mut out_rx) = spawn_core(test_ctx(&backend, 8));
        in_tx
            .send(Envelope::new(Op::Dial, 3, crate::wire::dial_body(&backend)))
            .await
            .unwrap();
        recv_op(&mut out_rx, Op::OpenOk).await;
        recv_op(&mut out_rx, Op::Close).await;
    }

    #[tokio::test]
    async fn session_cap_rejects_new_dials_but_keeps_existing() {
        let (echo_addr, stop_echo) = spawn_echo_server().await;
        let backend = echo_addr.to_string();
        let (in_tx, mut out_rx) = spawn_core(test_ctx(&backend, 1));

        // 第一个 session 成功
        in_tx.send(Envelope::new(Op::Dial, 1, crate::wire::dial_body(&backend))).await.unwrap();
        recv_op(&mut out_rx, Op::OpenOk).await;

        // 第二个被拒
        in_tx.send(Envelope::new(Op::Dial, 2, crate::wire::dial_body(&backend))).await.unwrap();
        let err = recv_op(&mut out_rx, Op::OpenErr).await;
        assert_eq!(err.session, 2);
        assert_eq!(err.body, b"session cap exceeded");

        // 关掉第一个后再 DIAL 成功（下行 CLOSE 已摘除 session 1）
        in_tx.send(Envelope::new(Op::Close, 1, Vec::new())).await.unwrap();
        in_tx.send(Envelope::new(Op::Dial, 3, crate::wire::dial_body(&backend))).await.unwrap();
        recv_op(&mut out_rx, Op::OpenOk).await;

        in_tx.send(Envelope::new(Op::Close, 3, Vec::new())).await.unwrap();
        stop_echo.send(()).ok();
    }

    #[tokio::test]
    async fn data_after_close_is_dropped_not_panic() {
        let (echo_addr, stop_echo) = spawn_echo_server().await;
        let backend = echo_addr.to_string();
        let (in_tx, mut out_rx) = spawn_core(test_ctx(&backend, 8));
        in_tx.send(Envelope::new(Op::Dial, 5, crate::wire::dial_body(&backend))).await.unwrap();
        recv_op(&mut out_rx, Op::OpenOk).await;
        in_tx.send(Envelope::new(Op::Close, 5, Vec::new())).await.unwrap();
        // 迟到 DATA：静默丢弃
        in_tx.send(Envelope::new(Op::Data, 5, b"late".to_vec())).await.unwrap();
        // PONG/上行 OPEN_OK 无副作用
        in_tx.send(Envelope::new(Op::Pong, 0, Vec::new())).await.unwrap();
        in_tx.send(Envelope::new(Op::OpenOk, 5, Vec::new())).await.unwrap();
        // 循环仍在运行：再 PING 应有 PONG
        in_tx.send(Envelope::new(Op::Ping, 1, Vec::new())).await.unwrap();
        recv_op(&mut out_rx, Op::Pong).await;
        stop_echo.send(()).ok();
    }

    #[tokio::test]
    async fn inbound_channel_close_ends_core() {
        let (in_tx, out_rx) = spawn_core(test_ctx("127.0.0.1:22", 8));
        drop(in_tx);
        // 出站通道随 core 退出最终被丢弃；这里仅验证不挂死
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let mut rx = out_rx;
            while rx.recv().await.is_some() {}
        })
        .await;
    }

    #[tokio::test]
    async fn silence_detection_triggers_death() {
        // silence 极小：无入站报文时心跳 tick 判死退出
        let ctx = CoreCtx {
            local_backend: "127.0.0.1:22".into(),
            dial_timeout: Duration::from_secs(1),
            max_sessions: 8,
            heartbeat: Duration::from_millis(20),
            silence: Duration::from_millis(1),
        };
        let (in_tx, in_rx) = mpsc::channel::<Envelope>(4);
        let (out_tx, mut out_rx) = mpsc::channel::<Envelope>(4);
        let core = tokio::spawn(agent_core(in_rx, out_tx, ctx));
        // 第一轮 PING 出站后 core 判死退出
        let ping = out_rx.recv().await.expect("initial ping");
        assert_eq!(ping.op, Op::Ping);
        let _ = tokio::time::timeout(Duration::from_secs(2), core).await.expect("core exits on silence");
        drop(in_tx);
    }
}
