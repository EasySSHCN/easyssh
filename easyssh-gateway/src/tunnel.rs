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
    http::StatusCode,
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
    peer: SocketAddr,
    error: Option<String>,
) -> AuditEvent {
    let mut ev = AuditEvent::new(
        claims.sub.clone(),
        Some(canonical_role(&claims.roles).as_str()),
        event,
        resource_id,
        peer.ip().to_string(),
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
    Extension(claims): Extension<Claims>,
) -> Response {
    if resource_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "resource_id 不能为空").into_response();
    }

    // 0. 策略矩阵检查：资源 × 角色 × connect；拒绝 => 403 + policy_denied 审计
    if state
        .policy
        .check(&resource_id, canonical_role(&claims.roles), Action::Connect)
        == Decision::Deny
    {
        tracing::warn!(%peer, %resource_id, user = %claims.sub, "tunnel denied by policy matrix");
        state.audit.record(audit_event(
            &claims,
            "policy_denied",
            &resource_id,
            peer,
            Some("connect denied by policy matrix".into()),
        ));
        return (StatusCode::FORBIDDEN, "策略矩阵拒绝该连接（policy_denied）").into_response();
    }

    // 1. 解析后端地址并做角色级授权（注册表优先；其次显式私网 host:port 直连）
    let backend = match state.resources.resolve_backend(&resource_id, &claims.roles) {
        BackendResolution::Allowed(backend) => backend,
        BackendResolution::Forbidden => {
            tracing::warn!(%peer, %resource_id, user = %claims.sub, "tunnel forbidden by role policy");
            state.audit.record(audit_event(
                &claims,
                "tunnel_forbidden",
                &resource_id,
                peer,
                Some("roles not allowed by resource allowed_roles".to_string()),
            ));
            return (StatusCode::FORBIDDEN, "当前角色无权访问该资源").into_response();
        }
        BackendResolution::NotFound => {
            state.audit.record(audit_event(
                &claims,
                "tunnel_rejected",
                &resource_id,
                peer,
                Some("backend unknown or not in allowed private ranges".to_string()),
            ));
            return (StatusCode::NOT_FOUND, "资源不存在或后端地址不允许").into_response();
        }
    };

    // 2. 连接内网 SSH 主机（升级 WS 之前失败快速返回；显式 10s 超时）
    let tcp = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        TcpStream::connect(&backend),
    )
    .await
    {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => {
            let summary = format!("connect {backend} failed: {e}");
            tracing::warn!(%peer, %resource_id, %backend, error = %e, "backend connect failed");
            state.audit.record(audit_event(
                &claims,
                "tunnel_backend_error",
                &resource_id,
                peer,
                Some(summary),
            ));
            return (StatusCode::BAD_GATEWAY, "连接后端 SSH 主机失败").into_response();
        }
        Err(_) => {
            let summary = format!("connect {backend} timed out after 10s");
            state.audit.record(audit_event(
                &claims,
                "tunnel_backend_error",
                &resource_id,
                peer,
                Some(summary),
            ));
            return (StatusCode::GATEWAY_TIMEOUT, "连接后端 SSH 主机超时").into_response();
        }
    };

    let user = claims.sub.clone();
    let role = canonical_role(&claims.roles).as_str().to_string();
    ws.on_upgrade(move |socket| {
        handle_tunnel_socket(state, resource_id, backend, user, role, peer, socket, tcp)
    })
}

/// 隧道会话：审计 open -> 双向泵 -> 审计 close。
#[allow(clippy::too_many_arguments)]
async fn handle_tunnel_socket(
    state: Arc<AppState>,
    resource_id: String,
    backend: String,
    user: String,
    role: String,
    peer: SocketAddr,
    socket: WebSocket,
    tcp: TcpStream,
) {
    tracing::info!(%peer, %resource_id, %backend, "tunnel established");
    state.audit.record(AuditEvent::new(
        user.clone(),
        Some(&role),
        "tunnel_open",
        resource_id.clone(),
        peer.ip().to_string(),
    ));

    let (ws_tx, ws_rx) = socket.split();
    let (tcp_r, tcp_w) = tokio::io::split(tcp);

    let (client_tx, client_rx) = mpsc::channel::<Message>(64);
    let (backend_tx, backend_rx) = mpsc::channel::<Message>(64);

    tokio::spawn(ws_source_to_channel(ws_rx, client_tx));
    let sink_task = tokio::spawn(channel_to_ws_sink(backend_rx, ws_tx));

    let to_backend = tokio::spawn(pump_ws_to_tcp(client_rx, tcp_w));
    let to_client = tokio::spawn(pump_tcp_to_ws(tcp_r, backend_tx));
    let _ = tokio::join!(to_backend, to_client);

    let _ = sink_task.await;

    state.audit.record(AuditEvent::new(
        user,
        Some(&role),
        "tunnel_close",
        resource_id,
        peer.ip().to_string(),
    ));
    tracing::info!(%peer, "tunnel closed");
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
}
