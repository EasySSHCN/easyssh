//! 审计模块：内存环形缓冲审计日志 + 查询接口。
//!
//! 审计事件包含：时间（Unix 秒）、用户、角色、事件、资源、来源 IP、结果。
//! 骨架阶段仅存内存（Mutex + 环形缓冲，最新在前对外），生产阶段应落地到
//! 不可篡改的日志后端（append-only 文件 / SIEM 推送）。
//!
//! 管理端点（X-Admin-Key 鉴权，R-20261120 首发内置）：
//! - GET /admin/audit?limit=N —— limit 钳制 1..=500，缺省 100，最新在前
//! - GET /admin/audit.csv     —— 同鉴权 CSV 导出
//! - POST /admin/issue        —— 服务端代签短期 token（回环调用形态）

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

use crate::AppState;

/// 单条审计事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// 事件时间（Unix 秒）
    pub timestamp: u64,
    /// 用户主体
    pub user: String,
    /// 用户角色（规范化 admin/user/readonly；系统事件缺省 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 事件名，如 tunnel_open / tunnel_close / auth_login / auth_placeholder_denied
    pub event: String,
    /// 目标资源 ID（无资源时为 "-"）
    pub resource: String,
    /// 客户端来源 IP
    pub source_ip: String,
    /// 错误摘要（失败事件；成功为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AuditEvent {
    /// 便捷构造：填时间/用户/角色/事件/资源/来源 IP，error = None。
    pub fn new(
        user: impl Into<String>,
        role: Option<&str>,
        event: impl Into<String>,
        resource: impl Into<String>,
        source_ip: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: now_unix(),
            user: user.into(),
            role: role.map(str::to_string),
            event: event.into(),
            resource: resource.into(),
            source_ip: source_ip.into(),
            error: None,
        }
    }

    /// 事件结果字符串："ok" 或 "error:<摘要>"（过滤与 CSV 共用）。
    pub fn result(&self) -> String {
        match &self.error {
            None => "ok".to_string(),
            Some(err) => format!("error:{err}"),
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 内存环形缓冲审计日志。
pub struct AuditLog {
    buf: Mutex<RingBuffer>,
}

struct RingBuffer {
    items: Vec<AuditEvent>,
    head: usize, // 下一个写入位置
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buf: Mutex::new(RingBuffer {
                items: Vec::with_capacity(capacity),
                head: 0,
            }),
        }
    }

    /// 记录一条事件；缓冲满时覆盖最旧记录。
    pub fn record(&self, event: AuditEvent) {
        let mut buf = self.buf.lock().expect("audit mutex poisoned");
        let cap = buf.items.capacity();
        if buf.items.len() < cap {
            buf.items.push(event);
        } else {
            let head = buf.head;
            buf.items[head] = event;
        }
        buf.head = (buf.head + 1) % cap;
    }

    /// 按用户（user 字段精确匹配）移除关联审计事件，返回移除条数。
    /// 环形语义保持：删除后 snapshot（按 head 展开的写入顺序）依然正确。
    pub fn clear_by_user(&self, user: &str) -> usize {
        self.remove_where(|e| e.user == user)
    }

    /// 按 (timestamp, user) 精确匹配移除审计事件（后台「删除选中」批量操作），
    /// 返回移除条数。环形语义处理同 clear_by_user。
    pub fn delete_entries(&self, entries: &[(u64, String)]) -> usize {
        self.remove_where(|e| {
            entries
                .iter()
                .any(|(ts, u)| *ts == e.timestamp && *u == e.user)
        })
    }

    /// 通用移除（内部）：保留未匹配事件；从「环满」退化为「未满」时 head 归零。
    fn remove_where<F: Fn(&AuditEvent) -> bool>(&self, pred: F) -> usize {
        let mut buf = self.buf.lock().expect("audit mutex poisoned");
        let before = buf.items.len();
        buf.items.retain(|e| !pred(e));
        let removed = before - buf.items.len();
        if removed > 0 && buf.items.len() < buf.items.capacity() {
            // 从「环满」退化为「未满」时，head 必须归零，否则 snapshot 展开顺序错乱
            buf.head = 0;
        }
        removed
    }

    /// 按写入顺序返回当前缓冲内的事件（旧 -> 新）。
    pub fn snapshot(&self) -> Vec<AuditEvent> {
        let buf = self.buf.lock().expect("audit mutex poisoned");
        let mut out = Vec::with_capacity(buf.items.len());
        if buf.items.len() < buf.items.capacity() {
            out.extend(buf.items.iter().cloned());
        } else {
            let head = buf.head;
            out.extend_from_slice(&buf.items[head..]);
            out.extend_from_slice(&buf.items[..head]);
        }
        out
    }
}

/// 查询参数：可选按用户/事件/资源/结果过滤，limit 限制返回条数。
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    /// "ok" 或 "error:<摘要>"（裸 "error" 匹配全部错误事件）
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

impl AuditQuery {
    /// 统一过滤逻辑（JSON 与 CSV 共用）。
    pub fn matches(&self, e: &AuditEvent) -> bool {
        self.user.as_ref().map_or(true, |u| &e.user == u)
            && self.event.as_ref().map_or(true, |ev| &e.event == ev)
            && self.resource.as_ref().map_or(true, |r| &e.resource == r)
            && self
                .result
                .as_ref()
                .map_or(true, |res| match res.trim() {
                    "error" => e.error.is_some(),
                    other => &e.result() == other,
                })
    }
}

// ---------------------------------------------------------------------
// 管理密钥端点：GET /admin/audit?limit=N（X-Admin-Key 鉴权）
// ---------------------------------------------------------------------

/// 管理端点默认返回条数与上限（R-20261120：limit 钳制 1..=500）。
pub const ADMIN_AUDIT_DEFAULT_LIMIT: usize = 100;
pub const ADMIN_AUDIT_MAX_LIMIT: usize = 500;

/// 单条对外展示的审计行（最新在前）。
#[derive(Debug, Serialize)]
pub struct AdminAuditRow {
    pub timestamp: u64,
    pub user: String,
    pub role: String,
    pub event: String,
    pub source_ip: String,
    pub resource: String,
    pub result: String,
}

/// 取缓冲内经过滤并截断（最新在前）的事件列表（JSON 与 CSV 端点共用）。
fn filtered_admin_events(state: &AppState, q: &AuditQuery) -> Vec<AuditEvent> {
    let limit = q
        .limit
        .unwrap_or(ADMIN_AUDIT_DEFAULT_LIMIT)
        .clamp(1, ADMIN_AUDIT_MAX_LIMIT);

    let mut events: Vec<AuditEvent> = state
        .audit
        .snapshot()
        .into_iter()
        .filter(|e| q.matches(e))
        .collect();
    if events.len() > limit {
        events = events.split_off(events.len() - limit); // 取最新 limit 条
    }
    events.reverse(); // 最新在前
    events
}

fn event_to_row(e: AuditEvent) -> AdminAuditRow {
    let result = e.result();
    AdminAuditRow {
        timestamp: e.timestamp,
        role: e.role.unwrap_or_else(|| "-".to_string()),
        user: e.user,
        event: e.event,
        source_ip: e.source_ip,
        resource: e.resource,
        result,
    }
}

async fn admin_audit(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let rows: Vec<AdminAuditRow> = filtered_admin_events(&state, &q)
        .into_iter()
        .map(event_to_row)
        .collect();
    Json(serde_json::json!({
        "ok": true,
        "count": rows.len(),
        "events": rows
    }))
    .into_response()
}

/// GET /admin/audit.csv —— 同鉴权的 CSV 导出（RFC4180 转义）。
async fn admin_audit_csv(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AuditQuery>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let mut csv = String::from("timestamp,user,role,event,resource,source_ip,result,error\n");
    let events = filtered_admin_events(&state, &q); // 已是最新在前
    for e in events {
        let err = e.error.clone().unwrap_or_default();
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            e.timestamp,
            csv_field(&e.user),
            csv_field(e.role.as_deref().unwrap_or("-")),
            csv_field(&e.event),
            csv_field(&e.resource),
            csv_field(&e.source_ip),
            csv_field(&e.result()),
            csv_field(&err),
        ));
    }
    (
        [(axum::http::header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv,
    )
        .into_response()
}

/// RFC4180 字段转义：含逗号/引号/换行或公式前缀（= + - @ TAB）时整体加引号。
fn csv_field(s: &str) -> String {
    let starts_formula = matches!(s.chars().next(), Some('=' | '+' | '-' | '@' | '\t'));
    if starts_formula || s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r')
    {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 管理端点鉴权：admin_key 未配置 -> 503；X-Admin-Key 缺失/无效 -> 401。
/// P4 起同时供 dynres.rs（/admin/resource）等管理面模块复用。
pub fn check_admin_key_pub(state: &AppState, headers: &axum::http::HeaderMap) -> Option<Response> {
    check_admin_key(state, headers)
}

fn check_admin_key(state: &AppState, headers: &axum::http::HeaderMap) -> Option<Response> {
    let Some(key) = state.admin_key.as_deref() else {
        return Some(
            (StatusCode::SERVICE_UNAVAILABLE, "admin_key 未配置，管理端点已禁用").into_response(),
        );
    };
    let provided = headers
        .get("X-Admin-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided.is_empty() {
        return Some((StatusCode::UNAUTHORIZED, "缺少 X-Admin-Key").into_response());
    }
    if !constant_time_eq(provided.as_bytes(), key.as_bytes()) {
        return Some((StatusCode::UNAUTHORIZED, "X-Admin-Key 无效").into_response());
    }
    None
}

/// 恒定时间比较（简化实现，防时序侧信道）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// POST /admin/issue 请求体。
#[derive(Debug, serde::Deserialize)]
pub struct IssueRequest {
    /// 用户主体（1–64 字符）。
    pub sub: String,
    /// 角色列表（≤8 项；空列表按 ["user"] 处理）。
    #[serde(default)]
    pub roles: Vec<String>,
    /// 有效期秒数（可选；缺省 = [auth] token_ttl_seconds；上限 31536000s≈10年，对齐 EasyRDP GW-TTL）。
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

/// POST /admin/register-resource —— X-Admin-Key 鉴权的运行时资源注册端点（R-20261121-002 自动化）。
/// 服务台「登记资源」经本机回环调用：即时注册 id/backend 到资源注册表（内存，零重启）。
#[derive(serde::Deserialize)]
pub struct RegisterResourceRequest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// 角色授权；缺省 = 全放行（连接与否仍由策略矩阵管辖）。
    #[serde(default)]
    pub allowed_roles: Option<Vec<String>>,
}
fn default_port() -> u16 { 22 }

async fn admin_register_resource(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<RegisterResourceRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let id = req.id.trim().to_string();
    if id.is_empty() || id.len() > 128 {
        return (StatusCode::BAD_REQUEST, "id 为空或超长（1-128 字符）").into_response();
    }
    let host = req.host.trim();
    let is_ip = host.parse::<std::net::IpAddr>().is_ok();
    let is_domain = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    if !is_ip && !is_domain {
        return (StatusCode::BAD_REQUEST, "host 非法（域名或 IP）").into_response();
    }
    let name = req.name.clone().unwrap_or_default();
    let name = if name.trim().is_empty() { id.clone() } else { name.trim().to_string() };
    let backend = format!("{host}:{}", req.port);
    state.resources.register_full(&id, &name, backend.clone(), req.allowed_roles.clone());
    let mut ev = AuditEvent::new("admin", Some("admin"), "admin_register_resource", &id, admin_source_ip(&headers));
    ev.error = Some(format!("backend={backend}"));
    state.audit.record(ev);
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "ok": true,
            "message": "资源已注册（运行时生效，无需重启）",
            "id": id,
            "backend": backend,
        })),
    )
        .into_response()
}

/// 管理通道审计来源 IP：优先取官网代理（PHP X-Admin-Key 回环调用）转发的
/// 管理员真实公网 IP（X-Admin-Client-IP）；缺失或非法时统一记为 "admin"，
/// 表示"来自管理通道、代理未提供来源"，不再出现含义不一致的 "-"。
pub(crate) fn admin_source_ip(headers: &axum::http::HeaderMap) -> String {
    if let Some(v) = headers.get("x-admin-client-ip").and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        let ok = !v.is_empty()
            && v.len() <= 45
            && v.bytes().all(|b| b.is_ascii_hexdigit() || b == b'.' || b == b':');
        if ok {
            return v.to_string();
        }
    }
    "admin".to_string()
}

/// POST /admin/audit-clear —— X-Admin-Key 鉴权：清空指定用户（user 字段精确匹配）
/// 的全部关联审计事件，并保留一条 audit_cleared 删除审计（用于账号删除/注销场景）。
#[derive(serde::Deserialize)]
pub struct AuditClearRequest {
    pub user: String,
}

async fn admin_audit_clear(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<AuditClearRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let user = req.user.trim().to_string();
    if user.is_empty() || user.len() > 254 {
        return (StatusCode::BAD_REQUEST, "user 为空或超长").into_response();
    }
    let removed = state.audit.clear_by_user(&user);
    let mut ev = AuditEvent::new("admin", Some("admin"), "audit_cleared", &user, admin_source_ip(&headers));
    ev.error = Some(format!("account-deleted cleared={removed}"));
    state.audit.record(ev);
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "ok": true, "removed": removed })),
    )
        .into_response()
}

/// POST /admin/audit-delete —— X-Admin-Key 鉴权：按 (timestamp, user) 精确删除
/// 后台勾选的审计事件（EasySSH admin「删除选中」批量操作）。
#[derive(serde::Deserialize)]
pub struct AuditDeleteRequest {
    pub entries: Vec<AuditDeleteEntry>,
}
#[derive(serde::Deserialize)]
pub struct AuditDeleteEntry {
    pub timestamp: u64,
    pub user: String,
}

async fn admin_audit_delete(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<AuditDeleteRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    if req.entries.is_empty() || req.entries.len() > 500 {
        return (StatusCode::BAD_REQUEST, "entries 为空或超限（1-500）").into_response();
    }
    let list: Vec<(u64, String)> = req
        .entries
        .iter()
        .map(|e| (e.timestamp, e.user.trim().to_string()))
        .collect();
    let removed = state.audit.delete_entries(&list);
    let mut ev = AuditEvent::new("admin", Some("admin"), "audit_deleted", "-", admin_source_ip(&headers));
    ev.error = Some(format!("requested={} removed={}", req.entries.len(), removed));
    state.audit.record(ev);
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "ok": true, "removed": removed })),
    )
        .into_response()
}
/// POST /admin/audit/record 请求体：{ user, event, resource? }。
/// （与 EasyRDP easyrdp-gateway 同口径：官网服务端代用户写入一条审计事件，
///  如 register/login/email_verified/resource_auto_granted/user_self_deleted；
///  X-Admin-Key 鉴权，仅限本机回环调用。）
#[derive(serde::Deserialize)]
pub struct AuditRecordRequest {
    pub user: String,
    pub event: String,
    #[serde(default)]
    pub resource: Option<String>,
}

/// POST /admin/audit/record —— 官网服务端代用户写入一条审计事件
/// （X-Admin-Key 鉴权，回环调用；留痕失败由调用方忽略，不影响主流程）。
async fn admin_audit_record(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<AuditRecordRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let user = req.user.trim().to_string();
    let event = req.event.trim().to_string();
    if user.is_empty() || user.len() > 64 || event.is_empty() || event.len() > 64 {
        return (StatusCode::BAD_REQUEST, "user/event 为空或超长（1-64 字符）").into_response();
    }
    let mut ev = AuditEvent::new(
        user,
        Some("user"),
        event,
        req.resource.unwrap_or_else(|| "-".to_string()),
        admin_source_ip(&headers),
    );
    ev.error = None;
    state.audit.record(ev);
    (StatusCode::OK, axum::Json(serde_json::json!({"ok": true, "message": "已记录"}))).into_response()
}

/// POST /admin/issue —— X-Admin-Key 鉴权的内部签发端点。仅供可信服务端
/// 在本机回环调用（网关绑定 127.0.0.1、反代不转发 /admin/*）。
async fn admin_issue(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<IssueRequest>,
) -> Response {
    if let Some(resp) = check_admin_key(&state, &headers) {
        return resp;
    }
    let sub = req.sub.trim();
    if sub.is_empty() || sub.len() > 64 {
        return (StatusCode::BAD_REQUEST, "sub 为空或超长（1-64 字符）").into_response();
    }
    if req.roles.len() > 8 {
        return (StatusCode::BAD_REQUEST, "roles 数量超限（≤8）").into_response();
    }
    let roles: Vec<String> = req
        .roles
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= 32)
        .collect();
    let roles = if roles.is_empty() { vec!["user".to_string()] } else { roles };
    // GW-TTL-20261122：放开 1h 钳制，允许用户自助签发长期 token（上限≈10年，与 EasyRDP 网关口径一致）
    let ttl = req.ttl_seconds.unwrap_or(0).min(31_536_000);
    let issued = if ttl == 0 {
        state.auth.issue_token(sub, &roles.iter().map(String::as_str).collect::<Vec<_>>())
    } else {
        state.auth.issue_token_with_ttl(
            sub,
            &roles.iter().map(String::as_str).collect::<Vec<_>>(),
            ttl,
        )
    };
    let token = match issued {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败").into_response(),
    };
    let expires_in = if ttl == 0 { state.auth.default_ttl_seconds() } else { ttl };
    state.audit.record(crate::audit::AuditEvent::new(
        sub,
        Some(crate::policy::canonical_role(&roles).as_str()),
        "admin_issue",
        "-",
        {
            let via_admin = admin_source_ip(&headers);
            if via_admin != "admin" {
                via_admin
            } else {
                headers
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("admin")
                    .to_string()
            }
        },
    ));
    Json(serde_json::json!({
        "ok": true,
        "token": token,
        "expires_in": expires_in,
        "token_type": "Bearer"
    }))
    .into_response()
}

/// 管理密钥路由：/admin/*（不走 JWT auth_guard，凭 X-Admin-Key 鉴权）。
pub fn admin_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/audit", get(admin_audit))
        .route("/admin/audit.csv", get(admin_audit_csv))
        .route("/admin/issue", post(admin_issue))
        .route("/admin/audit/record", post(admin_audit_record))
            .route("/admin/register-resource", post(admin_register_resource))
            .route("/admin/audit-clear", post(admin_audit_clear))
            .route("/admin/audit-delete", post(admin_audit_delete))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceRegistry;
    use axum::http::{Request, StatusCode as HttpCode};
    use tower::ServiceExt;

    fn event(user: &str, event: &str) -> AuditEvent {
        AuditEvent {
            timestamp: 1_700_000_000,
            user: user.to_string(),
            role: Some("user".to_string()),
            event: event.to_string(),
            resource: "ssh-jump01".to_string(),
            source_ip: "10.0.0.5".to_string(),
            error: None,
        }
    }

    #[test]
    fn ring_buffer_overwrites_oldest_when_full() {
        let log = AuditLog::new(3);
        for i in 0..5 {
            log.record(event(&format!("u{i}"), "tunnel_open"));
        }
        let snap = log.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].user, "u2");
        assert_eq!(snap[2].user, "u4");
    }

    #[test]
    fn csv_field_escapes_injection() {
        assert_eq!(csv_field("alice"), "alice");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=cmd"), "\"=cmd\"");
    }

    fn admin_state(admin_key: Option<&str>) -> Arc<AppState> {
        Arc::new(AppState {
            auth: crate::auth::AuthState::new("test-secret", 600),
            audit: AuditLog::new(64),
            resources: ResourceRegistry::from_configs(&[]),
            admin_key: admin_key.map(|k| Arc::new(k.to_string())),
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: None,
            dev_mode: false,
            dev_cidrs: Default::default(),
        agent_hub: crate::agent::AgentHub::new(),
        agent_cfg: Default::default(),
        agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        })
    }

    async fn call_admin_audit(
        state: &Arc<AppState>,
        key: Option<&str>,
        limit: Option<usize>,
    ) -> (HttpCode, serde_json::Value) {
        let mut uri = "/admin/audit".to_string();
        if let Some(l) = limit {
            uri.push_str(&format!("?limit={l}"));
        }
        let mut req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("build request");
        if let Some(k) = key {
            req.headers_mut()
                .insert("X-Admin-Key", k.parse().expect("header value"));
        }
        let app = admin_router(Arc::clone(state)).with_state(Arc::clone(state));
        let resp = app.oneshot(req).await.expect("oneshot");
        let code = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.expect("body");
        let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!(null));
        (code, json)
    }

    #[tokio::test]
    async fn admin_audit_rejects_missing_and_wrong_key() {
        let state = admin_state(Some("correct-key"));
        let (code, _) = call_admin_audit(&state, None, None).await;
        assert_eq!(code, HttpCode::UNAUTHORIZED);
        let (code, _) = call_admin_audit(&state, Some("wrong-key"), None).await;
        assert_eq!(code, HttpCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_audit_disabled_without_key() {
        let state = admin_state(None);
        let (code, _) = call_admin_audit(&state, Some("anything"), None).await;
        assert_eq!(code, HttpCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn admin_audit_newest_first_and_limit_clamp() {
        let state = admin_state(Some("k1"));
        for i in 0..5 {
            state.audit.record(event(&format!("u{i}"), "tunnel_open"));
        }
        let (code, json) = call_admin_audit(&state, Some("k1"), Some(2)).await;
        assert_eq!(code, HttpCode::OK);
        assert_eq!(json["count"], serde_json::json!(2));
        assert_eq!(json["events"][0]["user"], serde_json::json!("u4"), "最新在前");
        assert_eq!(json["events"][1]["user"], serde_json::json!("u3"));
        // limit=0 钳制到 1
        let (_, json) = call_admin_audit(&state, Some("k1"), Some(0)).await;
        assert_eq!(json["count"], serde_json::json!(1));
        // 超上限钳制到 500（缓冲只有 5 条）
        let (_, json) = call_admin_audit(&state, Some("k1"), Some(999_999)).await;
        assert_eq!(json["count"], serde_json::json!(5));
        // 缺省 limit = 100
        let (_, json) = call_admin_audit(&state, Some("k1"), None).await;
        assert_eq!(json["count"], serde_json::json!(5));
        // 行字段齐全
        for row in json["events"].as_array().unwrap() {
            for field in ["timestamp", "user", "role", "event", "source_ip", "resource", "result"] {
                assert!(row.get(field).is_some(), "row missing {field}");
            }
        }
    }

    #[tokio::test]
    async fn admin_issue_and_filter_query() {
        let state = admin_state(Some("k1"));
        state.audit.record(event("alice", "tunnel_open"));
        let mut ev = event("bob", "auth_placeholder_denied");
        ev.error = Some("dev_mode=false".to_string());
        state.audit.record(ev);

        let req = Request::builder()
            .method("GET")
            .uri("/admin/audit?event=auth_placeholder_denied&limit=10")
            .header("X-Admin-Key", "k1")
            .body(axum::body::Body::empty())
            .unwrap();
        let app = admin_router(Arc::clone(&state)).with_state(Arc::clone(&state));
        let resp = app.oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], serde_json::json!(1));
        assert_eq!(json["events"][0]["user"], serde_json::json!("bob"));
        assert_eq!(json["events"][0]["result"], serde_json::json!("error:dev_mode=false"));

        // /admin/issue：签发可校验短 token + 审计留痕
        let req = Request::builder()
            .method("POST")
            .uri("/admin/issue")
            .header("X-Admin-Key", "k1")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"sub":"u7","roles":["user"],"ttl_seconds":300}"#))
            .unwrap();
        let app = admin_router(Arc::clone(&state)).with_state(Arc::clone(&state));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let claims = state.auth.validate_token(json["token"].as_str().unwrap()).unwrap();
        assert_eq!(claims.sub, "u7");
        assert_eq!(claims.exp - claims.iat, 300);
        assert!(state.audit.snapshot().iter().any(|e| e.event == "admin_issue" && e.user == "u7"));
    }

    #[tokio::test]
    async fn admin_register_resource_runtime_and_reject() {
        let state = admin_state(Some("k1"));
        let app = admin_router(Arc::clone(&state)).with_state(Arc::clone(&state));

        // 无密钥 → 401
        let req = Request::builder()
            .method("POST")
            .uri("/admin/register-resource")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"id":"r1","host":"10.0.0.9","port":22}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpCode::UNAUTHORIZED);

        // 正常注册 → 运行时立即可解析（resolve_backend），并留审计
        let req = Request::builder()
            .method("POST")
            .uri("/admin/register-resource")
            .header("X-Admin-Key", "k1")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"id":"r1","name":"R1","host":"10.0.0.9","port":2222}"#))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["backend"], serde_json::json!("10.0.0.9:2222"));
        match state.resources.resolve_backend("r1", &["user".to_string()]) {
            crate::resources::BackendResolution::Allowed(b) => assert_eq!(b, "10.0.0.9:2222"),
            other => panic!("expected allowed, got {:?}", other),
        }
        assert!(state.audit.snapshot().iter().any(|e| e.event == "admin_register_resource" && e.resource == "r1"));

        // 非法 host → 400
        let req = Request::builder()
            .method("POST")
            .uri("/admin/register-resource")
            .header("X-Admin-Key", "k1")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"id":"r2","host":"bad host!"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn admin_audit_delete_entries() {
        let state = admin_state(Some("k1"));
        state.audit.record(event("delme", "tunnel_open"));
        state.audit.record(event("keepme", "tunnel_open"));
        let ts = state
            .audit
            .snapshot()
            .iter()
            .find(|e| e.user == "delme")
            .map(|e| e.timestamp)
            .unwrap_or(0);

        let req = Request::builder()
            .method("POST")
            .uri("/admin/audit-delete")
            .header("X-Admin-Key", "k1")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                format!("{{\"entries\":[{{\"timestamp\":{},\"user\":\"delme\"}}]}}", ts),
            ))
            .unwrap();
        let app = admin_router(Arc::clone(&state)).with_state(Arc::clone(&state));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HttpCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["removed"], serde_json::json!(1));
        let users: Vec<String> = state
            .audit
            .snapshot()
            .iter()
            .map(|e| e.user.clone())
            .collect();
        assert!(!users.iter().any(|u| u == "delme"));
        assert!(users.iter().any(|u| u == "keepme"));
        assert!(state.audit.snapshot().iter().any(|e| e.event == "audit_deleted"));
    }
}
