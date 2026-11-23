//! 目标机 Agent 反向隧道（R-20261122-004 P1，EasySSH 独立实现）。
//!
//! 与 EasyRDP 侧共享的只有 wire spec 文本（agent-wire/1），代码互不引用：
//! WS Binary 帧头 `[type:1][session_id:8 BE][len:4 BE]` + payload；
//! 指令集 DIAL(0x01)/OPEN_OK(0x02)/OPEN_ERR(0x03)/DATA(0x04)/CLOSE(0x05)/
//! PING(0x06)/PONG(0x07)；心跳 30s 间隔 × 连续 3 次未收到 → 判死；
//! 单 Agent 连接并发 session ≤ 8（[agent] max_sessions_per_conn）。
//!
//! P1 交付：Envelope 编解码 + AgentHub（resource_id 索引、重连替换、
//! 静默清扫）+ 帧路由 + `GET /agent/connect`（独立校验：AgentToken
//! roles=["agent"] + rid 绑定，issuer 与用户 JWT 隔离）。
//! P2 tunnel.rs agent 分支复用 AgentHub 与会话表；P3 easyssh-agent crate
//! 按同一 spec 对接。

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use tokio::sync::mpsc;

use crate::{audit::AuditEvent, auth::Claims, AppState};

// ---------------------------------------------------------------------
// wire：agent-wire/1 编解码
// ---------------------------------------------------------------------

/// 帧头字节数：1 type + 8 session + 4 len。
pub const HEADER_BYTES: usize = 13;

/// 单帧 body 上限 4 MiB（SSH/scp/sftp 业务帧远小于此；仅防声明攻击）。
pub const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// agent-wire/1 指令码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Dial = 0x01,
    OpenOk = 0x02,
    OpenErr = 0x03,
    Data = 0x04,
    Close = 0x05,
    Ping = 0x06,
    Pong = 0x07,
}

impl Op {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(b: u8) -> Option<Op> {
        Some(match b {
            0x01 => Op::Dial,
            0x02 => Op::OpenOk,
            0x03 => Op::OpenErr,
            0x04 => Op::Data,
            0x05 => Op::Close,
            0x06 => Op::Ping,
            0x07 => Op::Pong,
            _ => return None,
        })
    }
}

/// 一条 agent-wire/1 报文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub op: Op,
    pub session: u64,
    pub body: Vec<u8>,
}

impl Envelope {
    pub fn new(op: Op, session: u64, body: impl Into<Vec<u8>>) -> Self {
        Self {
            op,
            session,
            body: body.into(),
        }
    }

    /// 序列化为完整 WS Binary 负载。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_BYTES + self.body.len());
        out.push(self.op.code());
        out.extend_from_slice(&self.session.to_be_bytes());
        out.extend_from_slice(&(self.body.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

/// 从连续字节流上切一条完整报文：`Full(报文, 消耗字节数)` 或 `NeedMore`。
/// 非法指令码或超限 body 声明返回 `Err`（调用方应丢弃该连接残余缓冲）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFault {
    UnknownOp(u8),
    BodyTooLong(usize),
}

pub fn cut_envelope(buf: &[u8]) -> Result<Option<(Envelope, usize)>, WireFault> {
    if buf.len() < HEADER_BYTES {
        return Ok(None);
    }
    let op = Op::from_code(buf[0]).ok_or(WireFault::UnknownOp(buf[0]))?;
    let session = u64::from_be_bytes(buf[1..9].try_into().expect("8 bytes"));
    let body_len = u32::from_be_bytes(buf[9..13].try_into().expect("4 bytes")) as usize;
    if body_len > BODY_LIMIT {
        return Err(WireFault::BodyTooLong(body_len));
    }
    let total = HEADER_BYTES + body_len;
    if buf.len() < total {
        return Ok(None);
    }
    let body = buf[HEADER_BYTES..total].to_vec();
    Ok(Some((Envelope::new(op, session, body), total)))
}

/// 客户端会话 ID 分配（每资源通道独立，从 1 起递增；0 保留给心跳）。
#[derive(Default)]
pub struct SessionSeq(AtomicU64);

impl SessionSeq {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }
}

// ---------------------------------------------------------------------
// AgentHub：resource_id -> 在线 Agent 槽位
// ---------------------------------------------------------------------

/// 一条 Agent 连接在网关侧的槽位句柄。
pub struct AgentSlot {
    pub resource_id: String,
    pub agent_sub: String,
    /// 该资源在目标机本地的后端（Agent 仅可 DIAL 此地址）。
    pub local_backend: String,
    /// GW→Agent 出站队列（由 WS 出站泵编码发出）。
    pub to_agent: mpsc::Sender<Envelope>,
    /// Agent→GW 入站队列（分发循环持有，仅可取走一次）。
    from_agent: tokio::sync::Mutex<Option<mpsc::Receiver<Envelope>>>,
    /// 活跃客户端会话：session -> 回投通道。
    pub session_sinks: tokio::sync::Mutex<HashMap<u64, mpsc::Sender<Envelope>>>,
    pub session_cap: usize,
    seen_ms: AtomicU64,
}

impl AgentSlot {
    pub fn new(
        resource_id: impl Into<String>,
        agent_sub: impl Into<String>,
        local_backend: impl Into<String>,
        to_agent: mpsc::Sender<Envelope>,
        from_agent: mpsc::Receiver<Envelope>,
        session_cap: usize,
        now_ms: u64,
    ) -> Self {
        Self {
            resource_id: resource_id.into(),
            agent_sub: agent_sub.into(),
            local_backend: local_backend.into(),
            to_agent,
            from_agent: tokio::sync::Mutex::new(Some(from_agent)),
            session_sinks: tokio::sync::Mutex::new(HashMap::new()),
            session_cap: session_cap.max(1),
            seen_ms: AtomicU64::new(now_ms),
        }
    }

    pub async fn push(&self, env: Envelope) -> bool {
        self.to_agent.send(env).await.is_ok()
    }

    pub async fn take_from_agent(&self) -> Option<mpsc::Receiver<Envelope>> {
        self.from_agent.lock().await.take()
    }

    pub fn touch(&self, now_ms: u64) {
        self.seen_ms.store(now_ms, Ordering::Relaxed);
    }

    pub fn seen_ms(&self) -> u64 {
        self.seen_ms.load(Ordering::Relaxed)
    }

    /// 静默超过 `window_ms`（heartbeat × 3）即判死。
    pub fn is_silent(&self, now_ms: u64, window_ms: u64) -> bool {
        now_ms.saturating_sub(self.seen_ms.load(Ordering::Relaxed)) >= window_ms
    }
}

/// 在线 Agent 集合（同资源重连时以新槽位替换旧槽位）。
#[derive(Default)]
pub struct AgentHub {
    slots: RwLock<HashMap<String, Arc<AgentSlot>>>,
}

impl AgentHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach(&self, slot: AgentSlot) -> Arc<AgentSlot> {
        let slot = Arc::new(slot);
        self.slots
            .write()
            .expect("agent hub poisoned")
            .insert(slot.resource_id.clone(), Arc::clone(&slot));
        slot
    }

    pub fn detach(&self, resource_id: &str) -> Option<Arc<AgentSlot>> {
        self.slots
            .write()
            .expect("agent hub poisoned")
            .remove(resource_id)
    }

    pub fn slot(&self, resource_id: &str) -> Option<Arc<AgentSlot>> {
        self.slots
            .read()
            .expect("agent hub poisoned")
            .get(resource_id)
            .cloned()
    }

    pub fn is_online(&self, resource_id: &str) -> bool {
        self.slot(resource_id).is_some()
    }

    pub fn online_len(&self) -> usize {
        self.slots.read().expect("agent hub poisoned").len()
    }

    /// 清扫静默超时槽位，返回被摘除的 resource_id（重连替换过的新槽位不误摘）。
    pub fn prune_silent(&self, now_ms: u64, window_ms: u64) -> Vec<String> {
        let silent: Vec<String> = self
            .slots
            .read()
            .expect("agent hub poisoned")
            .iter()
            .filter(|(_, s)| s.is_silent(now_ms, window_ms))
            .map(|(rid, _)| rid.clone())
            .collect();
        if silent.is_empty() {
            return silent;
        }
        let mut slots = self.slots.write().expect("agent hub poisoned");
        silent
            .into_iter()
            .filter_map(|rid| slots.remove(&rid).map(|s| s.resource_id.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------
// 帧路由（纯逻辑，mpsc 假 Agent 单测覆盖）
// ---------------------------------------------------------------------

/// 入站报文路由结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteResult {
    /// 已向 Agent 回复（PING→PONG；PONG 无动作亦计入）。
    Answered(Op),
    /// 已投递到 session 回投通道。
    Forwarded(Op),
    /// 非法/不允许的上行报文（如 Agent 伪装 DIAL），丢弃。
    Ignored,
    /// session 未注册或已关闭。
    NoSink,
}

/// 路由一条 Agent→GW 入站报文：
/// PING → 回 PONG（同 session）；PONG → 心跳应答（无副作用）；
/// OPEN_OK/OPEN_ERR/DATA → 投递 session；CLOSE → 投递后摘除 session；
/// DIAL 为 GW→Agent 专属指令，Agent 上行一律丢弃。
pub async fn route_from_agent(slot: &AgentSlot, env: Envelope) -> RouteResult {
    match env.op {
        Op::Ping => {
            if slot
                .push(Envelope::new(Op::Pong, env.session, Vec::new()))
                .await
            {
                RouteResult::Answered(Op::Pong)
            } else {
                RouteResult::Ignored
            }
        }
        Op::Pong => RouteResult::Answered(Op::Pong),
        Op::OpenOk | Op::OpenErr | Op::Data | Op::Close => {
            let mut sinks = slot.session_sinks.lock().await;
            match sinks.get(&env.session) {
                Some(tx) => {
                    let ok = tx.send(env.clone()).await.is_ok();
                    if env.op == Op::Close || !ok {
                        sinks.remove(&env.session);
                    }
                    if ok {
                        RouteResult::Forwarded(env.op)
                    } else {
                        RouteResult::NoSink
                    }
                }
                None => RouteResult::NoSink,
            }
        }
        Op::Dial => RouteResult::Ignored,
    }
}

/// 为客户端会话登记回投通道；超上限（且非覆盖）拒绝。
pub async fn bind_session(slot: &AgentSlot, session: u64, tx: mpsc::Sender<Envelope>) -> bool {
    let mut sinks = slot.session_sinks.lock().await;
    if sinks.len() >= slot.session_cap && !sinks.contains_key(&session) {
        return false;
    }
    sinks.insert(session, tx);
    true
}

/// 摘除一条客户端会话（P2 tunnel 分支：DIAL 失败/会话结束时调用）。
pub async fn unbind_session(slot: &AgentSlot, session: u64) {
    slot.session_sinks.lock().await.remove(&session);
}

// ---------------------------------------------------------------------
// /agent/connect 路由
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ConnectQuery {
    pub resource_id: String,
}

/// 独立校验：AgentToken（issuer=easyssh-gateway-agent）+ roles 含 agent +
/// rid 与查询 resource_id 一致。用户 JWT 在 Agent 路径直接 401，反之亦然。
fn check_agent_auth(
    state: &AppState,
    headers: &HeaderMap,
    resource_id: &str,
) -> Result<Claims, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let claims = state
        .auth
        .validate_agent_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if !claims.roles.iter().any(|r| r == "agent") {
        return Err(StatusCode::FORBIDDEN);
    }
    if claims.rid.as_deref() != Some(resource_id) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(claims)
}

async fn agent_connect(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ConnectQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if q.resource_id.is_empty() {
        return (StatusCode::BAD_REQUEST, "resource_id 不能为空").into_response();
    }
    let ip = crate::console::client_ip_from_headers(&headers);

    if !state.agent_cfg.enabled {
        return (StatusCode::NOT_FOUND, "agent 通道未启用").into_response();
    }
    let claims = match check_agent_auth(&state, &headers, &q.resource_id) {
        Ok(c) => c,
        Err(code) => {
            let mut ev = AuditEvent::new("-", Some("agent"), "agent_auth_rejected", &q.resource_id, ip.clone());
            ev.error = Some(format!("status {code}"));
            state.audit.record(ev);
            return code.into_response();
        }
    };

    // 资源必须登记为 agent 型（静态或运行时层），Agent 接入才被放行。
    let local_backend = match state
        .resources
        .agent_backend_for(&q.resource_id, &claims.roles)
    {
        Some(b) => b,
        None => {
            let mut ev = AuditEvent::new(&claims.sub, Some("agent"), "agent_auth_rejected", &q.resource_id, ip.clone());
            ev.error = Some("not an agent-mode resource".into());
            state.audit.record(ev);
            return (StatusCode::NOT_FOUND, "资源不存在或非 agent 型").into_response();
        }
    };

    let resource_id = q.resource_id.clone();
    ws.on_upgrade(move |socket| {
        serve_agent_socket(state, resource_id, claims, local_backend, ip, socket)
    })
}

/// Agent WS 会话：登记 → 出站泵 → 入站解码分发 → 心跳计时 → 断开摘除 + 审计。
async fn serve_agent_socket(
    state: Arc<AppState>,
    resource_id: String,
    claims: Claims,
    local_backend: String,
    ip: String,
    socket: WebSocket,
) {
    let (mut sink, mut stream) = socket.split();
    let (to_agent, mut out_rx) = mpsc::channel::<Envelope>(64);
    let (from_tx, from_rx) = mpsc::channel::<Envelope>(64);
    let slot = state.agent_hub.attach(AgentSlot::new(
        resource_id.clone(),
        claims.sub.clone(),
        local_backend.clone(),
        to_agent,
        from_rx,
        state.agent_cfg.max_sessions_per_conn(),
        now_ms(),
    ));

    state.audit.record(AuditEvent::new(
        &claims.sub,
        Some("agent"),
        "agent_online",
        &resource_id,
        ip.clone(),
    ));
    // P4：动态资源 pending→online 联动（Agent 反向注册成功即翻在线）
    state.resources.dyn_set_status(&resource_id, "online");
    state.dyn_res.set_status(&resource_id, "online");
    tracing::info!(resource_id = %resource_id, agent = %claims.sub, "agent online");

    // 出站泵：GW→Agent 报文编码为 WS Binary。
    let out_task = tokio::spawn(async move {
        while let Some(env) = out_rx.recv().await {
            if sink.send(Message::Binary(env.to_bytes())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // 心跳计时器：网关驱动，每 heartbeat_secs 发 PING（session=0）。
    let hb_slot = Arc::clone(&slot);
    let hb_secs = state.agent_cfg.heartbeat_secs();
    let hb_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(hb_secs));
        tick.tick().await; // 跳过立即触发的首个 tick
        loop {
            tick.tick().await;
            if !hb_slot.push(Envelope::new(Op::Ping, 0, Vec::new())).await {
                break;
            }
        }
    });

    // 入站泵：Binary 跨帧缓冲解码 → from_tx；Text 忽略；解码失败丢弃残余。
    let in_task = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Binary(chunk) => {
                    buf.extend_from_slice(&chunk);
                    loop {
                        match cut_envelope(&buf) {
                            Ok(Some((env, used))) => {
                                buf.drain(..used);
                                if from_tx.send(env).await.is_err() {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(fault) => {
                                tracing::warn!(?fault, buffered = buf.len(), "agent wire fault");
                                return;
                            }
                        }
                    }
                }
                Message::Text(_) => {}
                Message::Close(_) => break,
                Message::Ping(_) | Message::Pong(_) => {}
            }
        }
    });

    // 分发循环：任何报文刷新静默计时；PING 自动回 PONG；业务报文投递 session。
    if let Some(mut rx) = slot.take_from_agent().await {
        while let Some(env) = rx.recv().await {
            slot.touch(now_ms());
            route_from_agent(&slot, env).await;
        }
    }

    // 摘除（仅当在线槽位仍是本连接）+ 审计下线。
    let removed = state.agent_hub.detach(&resource_id);
    if removed.map(|s| Arc::ptr_eq(&s, &slot)).unwrap_or(false) {
        state.audit.record(AuditEvent::new(
            &claims.sub,
            Some("agent"),
            "agent_offline",
            &resource_id,
            ip.clone(),
        ));
        // P4：离线联动（Agent 断线 = 该资源会话中断；登记台不被摘除，重连即恢复）
        state.resources.dyn_set_status(&resource_id, "offline");
        state.dyn_res.set_status(&resource_id, "offline");
        tracing::info!(resource_id = %resource_id, "agent offline");
    }
    out_task.abort();
    hb_task.abort();
    in_task.abort();
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------
// POST /agent/enroll（R-20261122-004 P4，方案 6.1.2 / P3 注记补位）
// ---------------------------------------------------------------------

/// AgentToken 有效期（24h；心跳静默续签 = 重新 enroll 或网关签发策略）。
const AGENT_TOKEN_TTL_SECS: u64 = 86400;

#[derive(Debug, Deserialize)]
pub struct EnrollRequest {
    pub enrollment_token: String,
}

/// 一次性 enrollment token 换 AgentToken：
/// 校验 token（存在 + 未过期 + 绑定动态资源）→ 签发 `roles=["agent"]`、
/// rid 绑定的 AgentToken（与用户 JWT issuer 隔离）→ 响应回读 resource_id。
/// token 一次性消费；资源保持 pending，Agent 随后经 `/agent/connect`
/// 反向注册成功即翻 `online`（见 serve_agent_socket 联动）。
pub async fn agent_enroll(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EnrollRequest>,
) -> Response {
    if !state.agent_cfg.enabled {
        return (StatusCode::NOT_FOUND, "agent 通道未启用").into_response();
    }
    let token = req.enrollment_token.trim().to_string();
    if token.is_empty() || token.len() > 128 {
        return (StatusCode::BAD_REQUEST, "enrollment_token 为空或超长").into_response();
    }
    let now = crate::dynres::now_ms();
    let grant = match state.enroll.consume(&token, now) {
        Some(g) => g,
        None => {
            let mut ev = AuditEvent::new("-", Some("agent"), "agent_enroll_rejected", "-", "-");
            ev.error = Some("invalid_or_expired_enrollment_token".into());
            state.audit.record(ev);
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "ok": false,
                    "message": "enrollment token 无效或已过期（一次性、10 分钟有效）；请在服务台重新登记获取",
                })),
            )
                .into_response();
        }
    };
    let rid = grant.resource_id.clone();
    // token 绑定的资源必须是动态层登记的 agent 型资源
    let entry = match state.dyn_res.get(&rid) {
        Some(e) if e.mode_parsed() == crate::resources::ResourceMode::Agent => e,
        _ => {
            let mut ev = AuditEvent::new(&grant.owner, Some("agent"), "agent_enroll_rejected", &rid, "-");
            ev.error = Some("not_agent_mode_resource".into());
            state.audit.record(ev);
            return (StatusCode::BAD_REQUEST, "enrollment token 绑定的资源非 agent 型").into_response();
        }
    };
    let agent_sub = format!("agent-{rid}");
    let agent_token = match state.auth.issue_agent_token(&agent_sub, &rid, AGENT_TOKEN_TTL_SECS) {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 AgentToken 失败").into_response(),
    };
    state.audit.record(AuditEvent::new(
        &entry.owner,
        Some("agent"),
        "agent_enroll",
        &rid,
        "-",
    ));
    tracing::info!(resource_id = %rid, owner = %entry.owner, "agent enroll exchanged");
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "agent_token": agent_token,
            "resource_id": rid,
            "expires_in": AGENT_TOKEN_TTL_SECS,
        })),
    )
        .into_response()
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/agent/connect", get(agent_connect))
        .route("/agent/enroll", post(agent_enroll))
}

// ---------------------------------------------------------------------
// 单元测试：wire 编解码 + 假 Agent（mpsc）+ 心跳判死
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn slot_pair(cap: usize) -> (Arc<AgentSlot>, mpsc::Receiver<Envelope>, mpsc::Sender<Envelope>) {
        let (tx, rx) = mpsc::channel(8);
        let (in_tx, in_rx) = mpsc::channel(8);
        let slot = Arc::new(AgentSlot::new(
            "ssh-intranet-01",
            "agent-u-0001",
            "127.0.0.1:22",
            tx,
            in_rx,
            cap,
            now_ms(),
        ));
        (slot, rx, in_tx)
    }

    #[test]
    fn envelope_roundtrip_covers_all_ops() {
        for op in [Op::Dial, Op::OpenOk, Op::OpenErr, Op::Data, Op::Close, Op::Ping, Op::Pong] {
            let env = Envelope::new(op, 0xA1B2_C3D4_E5F6_0789, b"ssh-bytes".to_vec());
            let raw = env.to_bytes();
            assert_eq!(raw.len(), HEADER_BYTES + 9);
            let (back, used) = cut_envelope(&raw).expect("decode").expect("full");
            assert_eq!(used, raw.len());
            assert_eq!(back, env);
            assert_eq!(back.op.code(), op.code());
        }
    }

    #[test]
    fn cut_envelope_handles_partial_and_pipelined_bytes() {
        let raw = Envelope::new(Op::Data, 7, b"abc".to_vec()).to_bytes();
        // header 不完整
        assert_eq!(cut_envelope(&raw[..4]).expect("ok"), None);
        // header 完整、body 截断
        assert_eq!(cut_envelope(&raw[..HEADER_BYTES + 2]).expect("ok"), None);
        // 粘包：双帧
        let mut doubled = raw.clone();
        doubled.extend_from_slice(&raw);
        let (_, used1) = cut_envelope(&doubled).expect("ok").expect("full");
        assert_eq!(used1, raw.len());
        let (_, used2) = cut_envelope(&doubled[used1..]).expect("ok").expect("full");
        assert_eq!(used2, raw.len());
    }

    #[test]
    fn cut_envelope_rejects_unknown_op_and_huge_body() {
        let mut raw = Envelope::new(Op::Data, 1, Vec::new()).to_bytes();
        raw[0] = 0xEE;
        assert_eq!(cut_envelope(&raw), Err(WireFault::UnknownOp(0xEE)));
        let mut big = Envelope::new(Op::Data, 1, Vec::new()).to_bytes();
        big[9..13].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            cut_envelope(&big),
            Err(WireFault::BodyTooLong(u32::MAX as usize))
        );
    }

    #[tokio::test]
    async fn dial_reaches_fake_agent_with_exact_bytes() {
        let (slot, mut out_rx, _in_tx) = slot_pair(8);
        let dial = Envelope::new(Op::Dial, 1, br#"{"backend":"127.0.0.1:22"}"#.to_vec());
        assert!(slot.push(dial.clone()).await);
        let got = out_rx.recv().await.expect("fake agent receives");
        assert_eq!(got, dial);
        assert_eq!(got.op, Op::Dial);
    }

    #[tokio::test]
    async fn ping_is_answered_with_pong_same_session() {
        let (slot, mut out_rx, _in_tx) = slot_pair(8);
        let r = route_from_agent(&slot, Envelope::new(Op::Ping, 99, Vec::new())).await;
        assert_eq!(r, RouteResult::Answered(Op::Pong));
        let pong = out_rx.recv().await.expect("pong");
        assert_eq!(pong.op, Op::Pong);
        assert_eq!(pong.session, 99);
    }

    #[tokio::test]
    async fn uplink_dial_from_agent_is_ignored() {
        let (slot, _out_rx, _in_tx) = slot_pair(8);
        let r = route_from_agent(&slot, Envelope::new(Op::Dial, 1, Vec::new())).await;
        assert_eq!(r, RouteResult::Ignored);
    }

    #[tokio::test]
    async fn data_open_err_close_flow_through_bound_session() {
        let (slot, _out_rx, _in_tx) = slot_pair(8);
        let (sink_tx, mut sink_rx) = mpsc::channel(8);
        assert!(bind_session(&slot, 5, sink_tx).await);

        // 未绑定 session
        let miss = route_from_agent(&slot, Envelope::new(Op::Data, 6, b"x".to_vec())).await;
        assert_eq!(miss, RouteResult::NoSink);

        // DATA
        assert_eq!(
            route_from_agent(&slot, Envelope::new(Op::Data, 5, b"SSH-2.0".to_vec())).await,
            RouteResult::Forwarded(Op::Data)
        );
        let got = sink_rx.recv().await.expect("frame");
        assert_eq!(got.body, b"SSH-2.0");

        // OPEN_ERR
        assert_eq!(
            route_from_agent(&slot, Envelope::new(Op::OpenErr, 5, b"conn refused".to_vec())).await,
            RouteResult::Forwarded(Op::OpenErr)
        );
        let err = sink_rx.recv().await.expect("frame");
        assert_eq!(err.op, Op::OpenErr);

        // CLOSE：投递后摘除 session
        assert_eq!(
            route_from_agent(&slot, Envelope::new(Op::Close, 5, Vec::new())).await,
            RouteResult::Forwarded(Op::Close)
        );
        let close = sink_rx.recv().await.expect("frame");
        assert_eq!(close.op, Op::Close);
        let late = route_from_agent(&slot, Envelope::new(Op::Data, 5, b"late".to_vec())).await;
        assert_eq!(late, RouteResult::NoSink);
    }

    #[tokio::test]
    async fn session_cap_and_rebind_semantics() {
        let (slot, _out_rx, _in_tx) = slot_pair(2);
        for sid in [11u64, 12] {
            let (tx, _rx) = mpsc::channel(4);
            assert!(bind_session(&slot, sid, tx).await, "sid {sid}");
        }
        let (tx3, _rx3) = mpsc::channel(4);
        assert!(!bind_session(&slot, 13, tx3).await, "超上限拒绝");
        // 覆盖同 sid 仍放行
        let (tx11, _rx11) = mpsc::channel(4);
        assert!(bind_session(&slot, 11, tx11).await);
    }

    #[test]
    fn silence_window_death_and_touch_reset() {
        let (tx, _rx) = mpsc::channel(4);
        let (in_tx, in_rx) = mpsc::channel(4);
        let slot = AgentSlot::new("r", "a", "127.0.0.1:22", tx, in_rx, 8, 1_000);
        // 30s × 3 = 90s 窗口
        assert!(!slot.is_silent(1_000 + 89_999, 90_000));
        assert!(slot.is_silent(1_000 + 90_000, 90_000));
        slot.touch(100_000);
        assert!(!slot.is_silent(150_000, 90_000));
        assert!(slot.is_silent(190_001, 90_000));
        let _ = in_tx; // 保持发送端存活以便构造
    }

    #[test]
    fn hub_prune_only_silent_slots() {
        let hub = AgentHub::new();
        let mk = |rid: &str, seen: u64| {
            let (tx, _rx) = mpsc::channel(4);
            let (_in_tx, in_rx) = mpsc::channel(4);
            AgentSlot::new(rid, "agent", "127.0.0.1:22", tx, in_rx, 8, seen)
        };
        hub.attach(mk("live", 1_000_000));
        hub.attach(mk("gone", 1_000_000));
        hub.slot("live").expect("live").touch(1_080_000);
        let removed = hub.prune_silent(1_091_000, 90_000); // gone: 91s；live: 11s
        assert_eq!(removed, vec!["gone".to_string()]);
        assert!(hub.is_online("live"));
        assert!(!hub.is_online("gone"));
        assert_eq!(hub.online_len(), 1);
    }

    #[test]
    fn hub_reconnect_replaces_slot_and_detach_matches_identity() {
        let hub = AgentHub::new();
        let mk = || {
            let (tx, _rx) = mpsc::channel(4);
            let (_in_tx, in_rx) = mpsc::channel(4);
            AgentSlot::new("r1", "agent", "127.0.0.1:22", tx, in_rx, 8, now_ms())
        };
        let first = hub.attach(mk());
        let second = hub.attach(mk());
        assert!(!Arc::ptr_eq(&first, &second), "重连替换");
        assert!(Arc::ptr_eq(&second, &hub.slot("r1").expect("slot")));
        assert_eq!(hub.online_len(), 1);
        let detached = hub.detach("r1").expect("detach");
        assert!(Arc::ptr_eq(&second, &detached));
        assert!(!hub.is_online("r1"));
    }

    #[test]
    fn session_seq_increments_from_one() {
        let seq = SessionSeq::new();
        assert_eq!(seq.take(), 1);
        assert_eq!(seq.take(), 2);
    }

    #[tokio::test]
    async fn fake_agent_full_dialog_dial_openok_data_close() {
        let (slot, mut out_rx, in_tx) = slot_pair(8);
        let (sink_tx, mut sink_rx) = mpsc::channel(8);
        assert!(bind_session(&slot, 1, sink_tx).await);
        let mut upstream = slot.take_from_agent().await.expect("inbound once");

        // GW→Agent: DIAL
        let dial = Envelope::new(Op::Dial, 1, br#"{"backend":"127.0.0.1:22"}"#.to_vec());
        assert!(slot.push(dial.clone()).await);
        assert_eq!(out_rx.recv().await.expect("dial"), dial);

        // Agent→GW: OPEN_OK
        in_tx.send(Envelope::new(Op::OpenOk, 1, Vec::new())).await.unwrap();
        let env = upstream.recv().await.expect("frame");
        slot.touch(now_ms());
        assert_eq!(env.op, Op::OpenOk);
        assert_eq!(route_from_agent(&slot, env).await, RouteResult::Forwarded(Op::OpenOk));

        // Agent→GW: DATA
        in_tx.send(Envelope::new(Op::Data, 1, b"banner".to_vec())).await.unwrap();
        let env = upstream.recv().await.expect("frame");
        slot.touch(now_ms());
        route_from_agent(&slot, env).await;
        let open = sink_rx.recv().await.expect("session frame");
        assert_eq!(open.op, Op::OpenOk);
        let data = sink_rx.recv().await.expect("session frame");
        assert_eq!(data.body, b"banner");

        // GW→Agent: DATA（客户端方向）
        let down = Envelope::new(Op::Data, 1, b"client->sshd".to_vec());
        assert!(slot.push(down.clone()).await);
        assert_eq!(out_rx.recv().await.expect("downlink"), down);

        // CLOSE 后迟到 DATA 不再路由
        in_tx.send(Envelope::new(Op::Close, 1, Vec::new())).await.unwrap();
        let env = upstream.recv().await.expect("frame");
        slot.touch(now_ms());
        assert_eq!(route_from_agent(&slot, env).await, RouteResult::Forwarded(Op::Close));
        let late = route_from_agent(&slot, Envelope::new(Op::Data, 1, b"x".to_vec())).await;
        assert_eq!(late, RouteResult::NoSink);
    }
}
