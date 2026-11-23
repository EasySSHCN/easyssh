//! 隧道模块：`GET /tunnel/{resource_id}` WebSocket 升级 + 真实 TCP 转发。
//!
//! 流程：auth_guard 校验短期 JWT -> 策略矩阵检查（资源 × 角色 × connect，
//! 拒绝 => 403 + policy_denied 审计）-> 依据 resource_id 解析后端 SSH 主机
//! 并做角色级授权（注册表优先 + allowed_roles 交集，403 = tunnel_forbidden；
//! 其次显式私网 "host:port"，SSRF 白名单见 resources.rs）-> 连接内网 SSH
//! 主机（升级前失败快速返回 502）-> WS 升级 -> 双向字节泵
//! （WS Binary 帧 <-> TCP 字节流）。建立、关闭、后端失败与角色拒绝均审计。
//!
//! 泵函数通过 mpsc 通道与 WS 分离，单测可用 mpsc 假 WS 直接验证转发语义。

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, State,
    },
    http::{HeaderMap, StatusCode},
    middleware::from_fn_with_state,
    response::{IntoResponse, Json, Response},
    routing::get,
    Extension, Router,
};
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use crate::{
    agent::{self, AgentSlot, Envelope, Op},
    audit::AuditEvent,
    auth::{auth_guard, Claims},
    policy::{canonical_role, Action, Decision},
    resources::{BackendResolution, ResourceInfo},
    AppState,
};

/// 构造带角色的审计事件（user/role 取自 JWT claims）。
fn audit_event(
    claims: &Claims,
    event: &str,
    resource_id: &str,
    peer_ip: &str,
    error: Option<String>,
) -> AuditEvent {
    let mut ev = AuditEvent::new(
        claims.sub.clone(),
        Some(canonical_role(&claims.roles).as_str()),
        event,
        resource_id,
        peer_ip.to_string(),
    );
    ev.error = error;
    ev
}

/// WS -> TCP 泵：Binary 帧字节写入 TCP 写半；Text 帧忽略；Close 结束并 FIN。
/// 返回是否正常收尾（true=EOF/Close，false=写入失败）。
pub async fn pump_ws_to_tcp<W>(mut rx: mpsc::Receiver<Message>, mut tcp_w: W) -> bool
where
    W: AsyncWrite + Unpin,
{
    while let Some(msg) = rx.recv().await {
        match msg {
            Message::Binary(data) => {
                if tcp_w.write_all(&data).await.is_err() {
                    return false;
                }
            }
            Message::Text(_) => {}
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
    let _ = tcp_w.shutdown().await;
    true
}

/// TCP -> WS 泵：读 TCP 字节切成 Binary 帧发往消息通道。
/// 返回是否读到后端 EOF 后正常结束（false=读错误或通道关闭）。
pub async fn pump_tcp_to_ws<R>(mut tcp_r: R, tx: mpsc::Sender<Message>) -> bool
where
    R: AsyncRead + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match tcp_r.read(&mut buf).await {
            Ok(0) => return true,
            Ok(n) => {
                if tx.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

/// 适配任务：axum 分离出的 WS 读半搬运进 mpsc 通道。
async fn ws_source_to_channel(
    mut ws_rx: futures_util::stream::SplitStream<WebSocket>,
    tx: mpsc::Sender<Message>,
) {
    while let Some(Ok(msg)) = ws_rx.next().await {
        if tx.send(msg).await.is_err() {
            break;
        }
    }
}

/// 适配任务：从 mpsc 通道取消息写回 WS 写半。
async fn channel_to_ws_sink(
    mut rx: mpsc::Receiver<Message>,
    mut ws_tx: futures_util::stream::SplitSink<WebSocket, Message>,
) {
    while let Some(msg) = rx.recv().await {
        if ws_tx.send(msg).await.is_err() {
            break;
        }
    }
}

async fn tunnel_handler(
    State(state): State<Arc<AppState>>,
    Path(resource_id): Path<String>,
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Extension(claims): Extension<Claims>,
) -> Response {
    // 反代信任链：回环对端时取 X-Real-IP（见 auth::effective_client_ip）
    let peer_ip = crate::auth::effective_client_ip(&peer, &headers);
    if resource_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "resource_id 不能为空").into_response();
    }

    // 0. 策略矩阵检查：资源 × 角色 × connect；拒绝 => 403 + policy_denied 审计
    if state
        .policy
        .check(&resource_id, canonical_role(&claims.roles), Action::Connect)
        == Decision::Deny
    {
        tracing::warn!(%peer_ip, %resource_id, user = %claims.sub, "tunnel denied by policy matrix");
        state.audit.record(audit_event(
            &claims,
            "policy_denied",
            &resource_id,
            &peer_ip,
            Some("connect denied by policy matrix".into()),
        ));
        return (StatusCode::FORBIDDEN, "策略矩阵拒绝该连接（policy_denied）").into_response();
    }

    // 1. 解析后端并做角色级授权（注册表优先；其次显式私网 host:port 直连；
    //    agent 型资源返回 Agent 分支，P2 走反向通道 DIAL）
    let backend = match state.resources.resolve_backend(&resource_id, &claims.roles) {
        BackendResolution::Allowed(addr) => BackendTarget::Direct(addr),
        BackendResolution::Agent { local_backend } => BackendTarget::AgentMode(local_backend),
        BackendResolution::Forbidden => {
            tracing::warn!(%peer_ip, %resource_id, user = %claims.sub, "tunnel forbidden by role policy");
            state.audit.record(audit_event(
                &claims,
                "tunnel_forbidden",
                &resource_id,
                &peer_ip,
                Some("roles not allowed by resource allowed_roles".to_string()),
            ));
            return (StatusCode::FORBIDDEN, "当前角色无权访问该资源").into_response();
        }
        BackendResolution::NotFound => {
            state.audit.record(audit_event(
                &claims,
                "tunnel_rejected",
                &resource_id,
                &peer_ip,
                Some("backend unknown or not in allowed private ranges".to_string()),
            ));
            return (StatusCode::NOT_FOUND, "资源不存在或后端地址不允许").into_response();
        }
    };

    // 2. 建立后端连接（升级 WS 之前失败快速返回）：
    //    - direct：网关主动 TcpStream::connect（10s 超时）；
    //    - agent：经反向通道 DIAL，等 OPEN_OK（dial_timeout_secs 超时）。
    let established = match &backend {
        BackendTarget::Direct(addr) => match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            TcpStream::connect(addr),
        )
        .await
        {
            Ok(Ok(t)) => EstablishedBackend::Tcp(t),
            Ok(Err(e)) => {
                let summary = format!("connect {addr} failed: {e}");
                tracing::warn!(%peer_ip, %resource_id, addr, error = %e, "backend connect failed");
                state.audit.record(audit_event(
                    &claims,
                    "tunnel_backend_error",
                    &resource_id,
                    &peer_ip,
                    Some(summary),
                ));
                return (StatusCode::BAD_GATEWAY, "连接后端 SSH 主机失败").into_response();
            }
            Err(_) => {
                let summary = format!("connect {addr} timed out after 10s");
                state.audit.record(audit_event(
                    &claims,
                    "tunnel_backend_error",
                    &resource_id,
                    &peer_ip,
                    Some(summary),
                ));
                return (StatusCode::GATEWAY_TIMEOUT, "连接后端 SSH 主机超时").into_response();
            }
        },
        BackendTarget::AgentMode(local_backend) => match agent_dial(
            &state, &resource_id, local_backend, &claims, &peer_ip,
        )
        .await
        {
            Ok(dialed) => dialed,
            Err(resp) => return resp,
        },
    };

    let user = claims.sub.clone();
    let role = canonical_role(&claims.roles).as_str().to_string();
    ws.on_upgrade(move |socket| {
        handle_tunnel_socket(state, resource_id, backend, user, role, peer_ip, socket, established)
    })
}

/// 后端接入形态（R-20261122-004 P2）。
enum BackendTarget {
    /// direct 型：网关主动连接的 "host:port"。
    Direct(String),
    /// agent 型：经目标机 Agent 反向隧道（local_backend 供 DIAL body）。
    AgentMode(String),
}

/// 已建立的后端连接。
enum EstablishedBackend {
    Tcp(TcpStream),
    Agent {
        slot: Arc<AgentSlot>,
        session: u64,
        sink_rx: mpsc::Receiver<Envelope>,
    },
}

/// agent 型资源 DIAL（P2）：分配 session 号 → bind 回投通道 → 向 Agent 发
/// DIAL → dial_timeout_secs 内等 OPEN_OK。失败（Agent 离线/会话上限/通道断/
/// OPEN_ERR/超时）返回错误响应并记录 agent_dial_error / agent_dial_timeout 审计。
async fn agent_dial(
    state: &AppState,
    resource_id: &str,
    local_backend: &str,
    claims: &Claims,
    peer_ip: &str,
) -> Result<EstablishedBackend, Response> {
    let audit_dial_err = |error: String| audit_event(claims, "agent_dial_error", resource_id, peer_ip, Some(error));

    if !state.agent_cfg.enabled {
        tracing::warn!(%peer_ip, %resource_id, "agent channel disabled");
        return Err((StatusCode::SERVICE_UNAVAILABLE, "agent 通道未启用").into_response());
    }
    let Some(slot) = state.agent_hub.slot(resource_id) else {
        tracing::warn!(%peer_ip, %resource_id, "agent not online");
        state.audit.record(audit_dial_err("agent offline".into()));
        return Err((StatusCode::BAD_GATEWAY, "目标机 Agent 不在线").into_response());
    };

    let session = state.agent_sessions.take();
    let (sink_tx, sink_rx) = mpsc::channel::<Envelope>(64);
    if !agent::bind_session(&slot, session, sink_tx).await {
        tracing::warn!(%peer_ip, %resource_id, "agent session limit reached");
        state.audit.record(audit_dial_err("session limit reached".into()));
        return Err((StatusCode::BAD_GATEWAY, "Agent 会话数已达上限").into_response());
    }

    let dial = Envelope::new(
        Op::Dial,
        session,
        serde_json::json!({ "backend": local_backend }).to_string().into_bytes(),
    );
    if !slot.push(dial).await {
        agent::unbind_session(&slot, session).await;
        tracing::warn!(%peer_ip, %resource_id, "agent outbound channel closed");
        state.audit.record(audit_dial_err("agent channel closed".into()));
        return Err((StatusCode::BAD_GATEWAY, "目标机 Agent 通道已断开").into_response());
    }

    let timeout_secs = state.agent_cfg.dial_timeout_secs();
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        wait_open(sink_rx),
    )
    .await;

    match opened {
        Ok(Ok(rx)) => {
            state.audit.record(audit_event(
                claims,
                "agent_dial",
                resource_id,
                peer_ip,
                None,
            ));
            tracing::info!(%peer_ip, %resource_id, session, "agent dial opened");
            Ok(EstablishedBackend::Agent { slot, session, sink_rx: rx })
        }
        Ok(Err(err_body)) => {
            agent::unbind_session(&slot, session).await;
            let summary = format!("agent open_err: {err_body}");
            tracing::warn!(%peer_ip, %resource_id, session, summary = %summary, "agent dial refused");
            state.audit.record(audit_dial_err(summary));
            Err((StatusCode::BAD_GATEWAY, "目标机 Agent 打开本地后端失败").into_response())
        }
        Err(_) => {
            agent::unbind_session(&slot, session).await;
            let summary = format!("agent dial timed out after {timeout_secs}s");
            tracing::warn!(%peer_ip, %resource_id, session, "agent dial timeout");
            state.audit.record(audit_event(
                claims,
                "agent_dial_timeout",
                resource_id,
                peer_ip,
                Some(summary),
            ));
            Err((StatusCode::GATEWAY_TIMEOUT, "目标机 Agent 响应超时").into_response())
        }
    }
}

/// 等待该 session 的 OPEN_OK / OPEN_ERR / CLOSE（视为拒绝）。
/// 成功返回带回投通道；失败返回错误 body 文本。
async fn wait_open(mut sink_rx: mpsc::Receiver<Envelope>) -> Result<mpsc::Receiver<Envelope>, String> {
    loop {
        match sink_rx.recv().await {
            Some(Envelope { op: Op::OpenOk, .. }) => return Ok(sink_rx),
            Some(Envelope { op: Op::OpenErr, body, .. }) => {
                return Err(String::from_utf8_lossy(&body).into_owned());
            }
            Some(Envelope { op: Op::Close, .. }) => return Err("closed before open".into()),
            Some(_) => {} // OPEN 前到达的其他报文：忽略（协议上不应出现）
            None => return Err("session channel closed".into()),
        }
    }
}

/// agent 会话双向适配（P2）：TCP 半边替换为 duplex + 帧适配任务，
/// 既有 pump_ws_to_tcp / pump_tcp_to_ws 泵函数零改动。
/// - 任务 A（Agent→客户端）：session 回投通道 DATA body 写入 duplex；
///   CLOSE/OPEN_ERR/通道关闭 = 后端 EOF（shutdown 写半）。
/// - 任务 B（客户端→Agent）：duplex 读半 16KiB 切帧发 DATA；EOF/错误发 CLOSE
///   并摘除 session。
/// 返回 duplex 的 gateway 视角读半、写半与任务 A 句柄，供既有泵直接消费。
fn spawn_agent_session_adapters(
    slot: Arc<AgentSlot>,
    session: u64,
    mut sink_rx: mpsc::Receiver<Envelope>,
) -> (
    tokio::io::ReadHalf<tokio::io::DuplexStream>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<()>,
) {
    let (gateway_side, backend_side): (
        tokio::io::DuplexStream,
        tokio::io::DuplexStream,
    ) = tokio::io::duplex(64 * 1024);
    let (backend_r, backend_w) = tokio::io::split(backend_side);

    // 任务 A：Agent 报文 → duplex（backend 写半）。
    let downstream = tokio::spawn(async move {
        let mut w = backend_w;
        loop {
            match sink_rx.recv().await {
                Some(Envelope { op: Op::Data, body, .. }) => {
                    if w.write_all(&body).await.is_err() {
                        return;
                    }
                }
                // CLOSE / OPEN_ERR / 通道关闭：后端 EOF 语义
                Some(_) | None => {
                    let _ = w.shutdown().await;
                    return;
                }
            }
        }
    });

    // 任务 B：duplex（backend 读半）→ Agent DATA；收尾发 CLOSE + 摘除 session。
    let _upstream = tokio::spawn(async move {
        let mut r = backend_r;
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match r.read(&mut buf).await {
                Ok(n) if n > 0 => {
                    let env = Envelope::new(Op::Data, session, buf[..n].to_vec());
                    if !slot.push(env).await {
                        break;
                    }
                }
                _ => break,
            }
        }
        let _ = slot
            .push(Envelope::new(Op::Close, session, Vec::new()))
            .await;
        agent::unbind_session(&slot, session).await;
    });

    let (gateway_r, gateway_w) = tokio::io::split(gateway_side);
    (gateway_r, gateway_w, downstream)
}

/// 隧道会话：审计 open -> 双向泵 -> 审计 close。
#[allow(clippy::too_many_arguments)]
async fn handle_tunnel_socket(
    state: Arc<AppState>,
    resource_id: String,
    _backend: BackendTarget,
    user: String,
    role: String,
    peer_ip: String,
    socket: WebSocket,
    established: EstablishedBackend,
) {
    tracing::info!(%peer_ip, %resource_id, "tunnel established");
    state.audit.record(AuditEvent::new(
        user.clone(),
        Some(&role),
        "tunnel_open",
        resource_id.clone(),
        peer_ip.clone(),
    ));

    let (ws_tx, ws_rx) = socket.split();
    let (client_tx, client_rx) = mpsc::channel::<Message>(64);
    let (backend_tx, backend_rx) = mpsc::channel::<Message>(64);

    tokio::spawn(ws_source_to_channel(ws_rx, client_tx));
    let sink_task = tokio::spawn(channel_to_ws_sink(backend_rx, ws_tx));

    // 后端读半/写半按接入形态构造；泵函数语义零改动。
    let (to_backend, to_client, downstream_agent) = match established {
        EstablishedBackend::Tcp(tcp) => {
            let (tcp_r, tcp_w) = tokio::io::split(tcp);
            (
                tokio::spawn(pump_ws_to_tcp(client_rx, tcp_w)),
                tokio::spawn(pump_tcp_to_ws(tcp_r, backend_tx)),
                None,
            )
        }
        EstablishedBackend::Agent { slot, session, sink_rx } => {
            let (backend_r, backend_w, downstream_agent) =
                spawn_agent_session_adapters(slot, session, sink_rx);
            (
                tokio::spawn(pump_ws_to_tcp(client_rx, backend_w)),
                tokio::spawn(pump_tcp_to_ws(backend_r, backend_tx)),
                Some(downstream_agent),
            )
        }
    };

    let _ = tokio::join!(to_backend, to_client);

    let _ = sink_task.await;
    if let Some(downstream_agent) = downstream_agent {
        // Agent 适配任务 A 可能仍阻塞在回投通道上（客户端先断开时），显式收尾。
        downstream_agent.abort();
    }

    state.audit.record(AuditEvent::new(
        user,
        Some(&role),
        "tunnel_close",
        resource_id,
        peer_ip.clone(),
    ));
    tracing::info!(%peer_ip, "tunnel closed");
}

/// GET /api/resources —— 资源列表（需认证；只含 id/name，不含 host/port）。
async fn list_resources(
    State(state): State<Arc<AppState>>,
    Extension(claims): Extension<Claims>,
) -> Response {
    // 策略矩阵检查：角色 × list（resource 维度以 "*" 通配判定）
    if state
        .policy
        .check("*", canonical_role(&claims.roles), Action::List)
        == Decision::Deny
    {
        return (StatusCode::FORBIDDEN, "策略矩阵拒绝资源列表访问").into_response();
    }
    let items: Vec<ResourceInfo> = state.resources.list();
    Json(serde_json::json!({ "resources": items })).into_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // 隧道路由挂 auth_guard：Bearer 校验失败即 401，WebSocket 升级不会发生。
    // 注意：axum 0.7 路径参数语法为 ":param"（0.8 起才是 "{param}"）
    Router::new()
        .route("/tunnel/:resource_id", get(tunnel_handler))
        .route("/api/resources", get(list_resources))
        .route_layer(from_fn_with_state(Arc::clone(&state), auth_guard))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 单元测试：泵函数转发语义（mpsc 假 WS + duplex 假 TCP）
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceRegistry;
    use tokio::io::duplex;

    #[tokio::test]
    async fn ws_to_tcp_pump_forwards_binary_ignores_text() {
        let (tx, rx) = mpsc::channel(8);
        let (tcp_client, mut tcp_server) = duplex(64);
        let pump = tokio::spawn(pump_ws_to_tcp(rx, tcp_client));
        tx.send(Message::Binary(vec![0x53, 0x53, 0x48])).await.unwrap();
        tx.send(Message::Text("ignored".into())).await.unwrap();
        tx.send(Message::Binary(b"payload".to_vec())).await.unwrap();
        drop(tx);
        assert!(pump.await.unwrap());
        let mut got = Vec::new();
        tcp_server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"\x53\x53\x48payload");
    }

    #[tokio::test]
    async fn ws_to_tcp_pump_reports_failure_when_write_errors() {
        let (tx, rx) = mpsc::channel(8);
        let (tcp_client, _dropped) = duplex(4);
        drop(_dropped);
        tx.send(Message::Binary(vec![1u8; 64])).await.unwrap();
        drop(tx);
        assert!(!pump_ws_to_tcp(rx, tcp_client).await);
    }

    #[tokio::test]
    async fn tcp_to_ws_pump_streams_binary_until_eof() {
        let (mut tcp_server, tcp_client) = duplex(64);
        let (tx, mut rx) = mpsc::channel(8);
        let pump = tokio::spawn(pump_tcp_to_ws(tcp_client, tx));
        tcp_server.write_all(b"SSH-BANNER").await.unwrap();
        tcp_server.shutdown().await.unwrap();
        assert!(pump.await.unwrap());
        let mut frames = Vec::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                Message::Binary(d) => frames.extend_from_slice(&d),
                Message::Text(_) => panic!("不应产生文本帧"),
                _ => {}
            }
        }
        assert_eq!(frames, b"SSH-BANNER");
    }

    #[tokio::test]
    async fn tcp_to_ws_pump_reports_failure_when_channel_closed() {
        let (mut tcp_writer, tcp_client) = duplex(64);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let pump = tokio::spawn(pump_tcp_to_ws(tcp_client, tx));
        tcp_writer.write_all(b"x").await.unwrap();
        assert!(!pump.await.unwrap());
    }

    #[test]
    fn resolve_rejects_public_backend_for_explicit_tunnel() {
        let mut c = crate::resources::ResourceConfig {
            id: "public-target".into(),
            name: "x".into(),
            host: "8.8.8.8".into(),
            port: 22,
            allowed_roles: None,
            mode: None,
            agent_local_backend: None,
        };
        let reg = ResourceRegistry::from_configs(&[c.clone()]);
        // 注册表是管理员配置的可信来源，仍可解析；
        // 但 resource_id 显式直连路径受 SSRF 白名单约束。
        assert_eq!(
            reg.resolve_backend("public-target", &[]),
            BackendResolution::Allowed("8.8.8.8:22".into())
        );
        c.host = "10.0.0.9".into();
        let reg2 = ResourceRegistry::from_configs(&[c]);
        assert_eq!(
            reg2.resolve_backend("10.0.0.9:22", &[]),
            BackendResolution::Allowed("10.0.0.9:22".into())
        );
        assert_eq!(reg2.resolve_backend("8.8.8.8:22", &[]), BackendResolution::NotFound);
    }

    // -----------------------------------------------------------------
    // P2（R-20261122-004）：agent 分支（独立实现，与 EasyRDP 仅共享 wire spec）
    // -----------------------------------------------------------------

    use crate::agent::{AgentHub, AgentSlot, SessionSeq};
    use crate::config::AgentConfig;

    fn p2_app_state() -> Arc<AppState> {
        let reg = ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
            id: "agent-host".into(),
            name: "内网 SSH 主机".into(),
            host: "-".into(),
            port: 0,
            allowed_roles: None,
            mode: Some("agent".into()),
            agent_local_backend: Some("127.0.0.1:2201".into()),
        }]);
        Arc::new(AppState {
            auth: crate::auth::AuthState::new("p2-test-secret", 3600),
            audit: crate::audit::AuditLog::new(64),
            resources: reg,
            admin_key: None,
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: None,
            dev_mode: false,
            dev_cidrs: Arc::new(crate::cidr::loopback_defaults()),
            agent_hub: AgentHub::new(),
            agent_cfg: AgentConfig {
                enabled: true,
                dial_timeout_secs: Some(1),
                ..Default::default()
            },
            agent_sessions: SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        })
    }

    fn p2_claims() -> Claims {
        Claims {
            sub: "bob".into(),
            roles: vec!["user".into()],
            iat: 0,
            exp: u64::MAX,
            iss: "easyssh-gateway".into(),
            rid: None,
        }
    }

    /// agent 型资源解析：resolve_backend 返回 Agent { local_backend }。
    #[test]
    fn resolve_agent_mode_resource_returns_agent_branch() {
        let reg = ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
            id: "agent-host".into(),
            name: "内网 SSH 主机".into(),
            host: "-".into(),
            port: 0,
            allowed_roles: None,
            mode: Some("agent".into()),
            agent_local_backend: Some("127.0.0.1:2201".into()),
        }]);
        assert_eq!(
            reg.resolve_backend("agent-host", &["user".to_string()]),
            BackendResolution::Agent {
                local_backend: "127.0.0.1:2201".into()
            }
        );
    }

    /// Agent 不在线：agent_dial 快速 502。
    #[tokio::test]
    async fn agent_dial_offline_returns_502() {
        let state = p2_app_state();
        let claims = p2_claims();
        let resp = agent_dial(&state, "agent-host", "127.0.0.1:2201", &claims, "127.0.0.1:5100")
            .await
            .err()
            .expect("offline must fail");
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    /// DIAL → OPEN_OK：会话建立，Agent 上行 DATA 路由进会话。
    #[tokio::test]
    async fn agent_dial_open_ok_establishes_session() {
        let state = p2_app_state();
        let claims = p2_claims();

        let (to_tx, mut to_rx) = mpsc::channel(16);
        let (from_tx, from_rx) = mpsc::channel(16);
        let slot = state.agent_hub.attach(AgentSlot::new(
            "agent-host",
            "agent-e2e",
            "127.0.0.1:2201",
            to_tx,
            from_rx,
            8,
            agent::now_ms(),
        ));

        // 先挂分发循环（OPEN_OK 需经 route_from_agent 投递到 session）；
        // agent_dial 放独立任务，避免单线程 runtime 下先 recv 等 DIAL 死锁。
        let mut inbound = slot.take_from_agent().await.expect("inbound once");
        let dispatch_slot = slot.clone();
        let dispatch = tokio::spawn(async move {
            while let Some(env) = inbound.recv().await {
                dispatch_slot.touch(agent::now_ms());
                agent::route_from_agent(&dispatch_slot, env).await;
            }
        });
        let st = Arc::clone(&state);
        let fut = tokio::spawn(async move {
            agent_dial(&st, "agent-host", "127.0.0.1:2201", &p2_claims(), "127.0.0.1:5101").await
        });
        let dial = to_rx.recv().await.expect("dial envelope");
        assert_eq!(dial.op, Op::Dial);
        let body = String::from_utf8(dial.body).unwrap();
        assert!(body.contains("127.0.0.1:2201"), "DIAL body = {body}");
        let sid = dial.session;
        assert!(sid > 0);

        from_tx.send(Envelope::new(Op::OpenOk, sid, Vec::new())).await.unwrap();

        let EstablishedBackend::Agent { slot: s, session, sink_rx } = fut.await.expect("task").expect("dial ok")
        else {
            panic!("expected agent established");
        };
        assert!(Arc::ptr_eq(&s, &slot));
        assert_eq!(session, sid);

        from_tx.send(Envelope::new(Op::Data, sid, b"SSH-2.0".to_vec())).await.unwrap();
        let mut sink_rx = sink_rx;
        let got = sink_rx.recv().await.expect("data routed to session");
        assert_eq!(got.body, b"SSH-2.0");
        dispatch.abort();
    }

    /// DIAL → OPEN_ERR：502 + 会话摘除。
    #[tokio::test]
    async fn agent_dial_open_err_returns_502_and_unbinds() {
        let state = p2_app_state();
        let claims = p2_claims();

        let (to_tx, mut to_rx) = mpsc::channel(16);
        let (from_tx, from_rx) = mpsc::channel(16);
        let slot = state.agent_hub.attach(AgentSlot::new(
            "agent-host",
            "agent-e2e",
            "127.0.0.1:2201",
            to_tx,
            from_rx,
            8,
            agent::now_ms(),
        ));

        // 分发循环：OPEN_ERR 需经 route_from_agent 投递到 session 才算"拒绝"
        let mut inbound = slot.take_from_agent().await.expect("inbound once");
        let dispatch_slot = slot.clone();
        let dispatch = tokio::spawn(async move {
            while let Some(env) = inbound.recv().await {
                dispatch_slot.touch(agent::now_ms());
                agent::route_from_agent(&dispatch_slot, env).await;
            }
        });
        // agent_dial 放到独立任务，否则单线程 runtime 下先 recv 等 DIAL 会死锁
        let st = Arc::clone(&state);
        let fut = tokio::spawn(async move {
            agent_dial(&st, "agent-host", "127.0.0.1:2201", &p2_claims(), "127.0.0.1:5102").await
        });
        let dial = to_rx.recv().await.expect("dial");
        from_tx.send(Envelope::new(Op::OpenErr, dial.session, b"refused".to_vec())).await.unwrap();
        let resp = fut.await.expect("task").err().expect("must fail");
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        assert!(
            slot.session_sinks.lock().await.is_empty(),
            "OPEN_ERR 后 session 必须摘除"
        );
        dispatch.abort();
    }

    /// DIAL 超时：504 + 会话摘除。
    #[tokio::test]
    async fn agent_dial_timeout_returns_504_and_unbinds() {
        let state = p2_app_state();
        let claims = p2_claims();

        let (to_tx, mut to_rx) = mpsc::channel(16);
        let (_from_tx, from_rx) = mpsc::channel(16);
        let slot = state.agent_hub.attach(AgentSlot::new(
            "agent-host",
            "agent-e2e",
            "127.0.0.1:2201",
            to_tx,
            from_rx,
            8,
            agent::now_ms(),
        ));

        let resp = agent_dial(&state, "agent-host", "127.0.0.1:2201", &claims, "127.0.0.1:5103")
            .await
            .err()
            .expect("must timeout");
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(slot.session_sinks.lock().await.is_empty(), "超时后 session 摘除");
        let _ = to_rx.recv().await; // 消费 DIAL
    }

    /// 回环 e2e（字节级）：适配器 + 既有双泵 + mpsc 假 Agent（SSH echo）全链路，
    /// 客户端写入与回读逐字节一致；客户端断开后 CLOSE 摘除会话。
    #[tokio::test]
    async fn agent_p2_loopback_bytes_identical_and_close_cleanup() {
        let state = p2_app_state();
        let claims = p2_claims();

        let (to_tx, mut to_rx) = mpsc::channel::<Envelope>(256);
        let (from_tx, from_rx) = mpsc::channel::<Envelope>(256);
        let slot = state.agent_hub.attach(AgentSlot::new(
            "agent-host",
            "agent-e2e",
            "127.0.0.1:22",
            to_tx,
            from_rx,
            8,
            agent::now_ms(),
        ));

        // 假 Agent：DIAL→OPEN_OK；DATA 原样回显（模拟 sshd echo）；CLOSE 发标记。
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel::<u64>();
        let mut closed_tx = Some(closed_tx);
        let echo_from_tx = from_tx.clone();
        let agent_task = tokio::spawn(async move {
            let mut open_sessions = std::collections::HashSet::new();
            while let Some(env) = to_rx.recv().await {
                match env.op {
                    Op::Dial => {
                        open_sessions.insert(env.session);
                        echo_from_tx
                            .send(Envelope::new(Op::OpenOk, env.session, Vec::new()))
                            .await
                            .unwrap();
                    }
                    Op::Data if open_sessions.contains(&env.session) => {
                        echo_from_tx
                            .send(Envelope::new(Op::Data, env.session, env.body))
                            .await
                            .unwrap();
                    }
                    Op::Close => {
                        open_sessions.remove(&env.session);
                        if let Some(tx) = closed_tx.take() {
                            let _ = tx.send(env.session);
                        }
                    }
                    _ => {}
                }
            }
        });

        // 网关侧分发循环（与 agent 连接 handler 同语义）。
        let dispatch_slot = slot.clone();
        let mut inbound = slot.take_from_agent().await.expect("inbound once");
        let dispatch_task = tokio::spawn(async move {
            while let Some(env) = inbound.recv().await {
                dispatch_slot.touch(agent::now_ms());
                agent::route_from_agent(&dispatch_slot, env).await;
            }
        });

        let EstablishedBackend::Agent { session, sink_rx, .. } =
            agent_dial(&state, "agent-host", "127.0.0.1:22", &claims, "127.0.0.1:5104")
                .await
                .expect("agent dial ok")
        else {
            panic!("expected agent established");
        };
        assert!(slot.session_sinks.lock().await.contains_key(&session));

        // 适配器 + 既有双泵（WS 半边用 mpsc 通道模拟）
        let (gw_r, gw_w, downstream) = spawn_agent_session_adapters(slot.clone(), session, sink_rx);
        let (client_tx, client_rx) = mpsc::channel::<Message>(64);
        let (backend_tx, mut backend_rx) = mpsc::channel::<Message>(64);
        let to_backend = tokio::spawn(pump_ws_to_tcp(client_rx, gw_w));
        let to_client = tokio::spawn(pump_tcp_to_ws(gw_r, backend_tx));

        // 客户端 → 目标机 → 回读：100KB 任意字节（强制多帧切分）
        let pattern: Vec<u8> = (0..100 * 1024u32).map(|i| (i % 251) as u8).collect();
        client_tx.send(Message::Binary(pattern.clone())).await.unwrap();

        let mut got = Vec::with_capacity(pattern.len());
        while got.len() < pattern.len() {
            match backend_rx.recv().await {
                Some(Message::Binary(d)) => got.extend_from_slice(&d),
                Some(_) => {}
                None => panic!("backend channel closed early at {} bytes", got.len()),
            }
        }
        assert_eq!(got, pattern, "经 Agent 反向隧道回环的字节流必须逐字节一致");

        // 客户端断开：CLOSE → 假 Agent 摘会话 → 回环静默收尾
        drop(client_tx);
        let _ = tokio::join!(to_backend, to_client);
        downstream.abort();

        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), closed_rx).await;
        assert!(closed.is_ok(), "适配器收尾必须向 Agent 发送 CLOSE");
        assert_eq!(closed.expect("closed").expect("sid"), session, "CLOSE 应带本会话号");
        assert!(
            slot.session_sinks.lock().await.is_empty(),
            "会话结束后必须摘除"
        );

        dispatch_task.abort();
        agent_task.abort();
    }
}
