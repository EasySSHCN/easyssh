//! 认证模块：短期 JWT 签发/校验 + Bearer 认证中间件 + OIDC 占位闸门 +
//! 真实 OIDC code 换票 + TOTP（RFC 6238）。
//!
//! 安全不变量（R-20261120-006/007/008，首发即内置）：
//! - **占位解析** `{"oidc_code":"用户:admin"}`：仅在 `dev_mode=true` 且来源 IP
//!   命中 CIDR 白名单时放行；否则 401 + `auth_placeholder_denied` 审计事件。
//!   `dev_mode` 默认 false（生产口径）。
//! - **真实 OIDC code 主路径**：不含 `:` 的 code 视为真实授权码，走
//!   discovery → token 换票 → id_token JWKS 签名校验 → claims 映射角色，
//!   不受占位闸门影响。
//! - 登录成功签发短有效期（默认 600s）JWT，作为网关预认证凭据（Bearer）。

use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::post,
    Router,
};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::AppState;

/// 默认 token 有效期：10 分钟。
pub const DEFAULT_TOKEN_TTL: u64 = 600;

/// JWT claims。`exp` 为 Unix 秒；jsonwebtoken 会自动校验过期。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// 用户主体（OIDC subject / 工号）
    pub sub: String,
    /// 角色列表（矩阵规范化为 admin/user/readonly 语义，见 policy::canonical_role）
    pub roles: Vec<String>,
    /// 签发时间（Unix 秒）
    pub iat: u64,
    /// 过期时间（Unix 秒）
    pub exp: u64,
    /// 签发方
    pub iss: String,
}

/// 认证状态：签名密钥与 token 有效期。
#[derive(Clone)]
pub struct AuthState {
    secret: Arc<String>,
    ttl: Duration,
    issuer: String,
}

impl AuthState {
    pub fn new(secret: impl Into<String>, ttl_seconds: u64) -> Self {
        Self {
            secret: Arc::new(secret.into()),
            ttl: Duration::from_secs(ttl_seconds.max(1)),
            issuer: "easyssh-gateway".to_string(),
        }
    }

    /// 当前生效的默认 token TTL 秒数。
    pub fn default_ttl_seconds(&self) -> u64 {
        self.ttl.as_secs()
    }

    fn claims(&self, subject: &str, roles: &[String], ttl: Duration) -> Claims {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Claims {
            sub: subject.to_string(),
            roles: roles.to_vec(),
            iat: now,
            exp: now + ttl.as_secs(),
            iss: self.issuer.clone(),
        }
    }

    fn encode(&self, claims: &Claims) -> Result<String, AuthError> {
        encode(&Header::default(), claims, &EncodingKey::from_secret(self.secret.as_bytes()))
            .map_err(|_| AuthError::IssueFailed)
    }

    /// 签发短期 JWT（默认 TTL）。
    pub fn issue_token(&self, subject: &str, roles: &[&str]) -> Result<String, AuthError> {
        let roles: Vec<String> = roles.iter().map(|s| s.to_string()).collect();
        self.encode(&self.claims(subject, &roles, self.ttl))
    }

    /// 按指定 TTL 签发 token（控制台会话 30min / admin issue 自定义）。
    pub fn issue_token_with_ttl(
        &self,
        subject: &str,
        roles: &[&str],
        ttl_seconds: u64,
    ) -> Result<String, AuthError> {
        let roles: Vec<String> = roles.iter().map(|s| s.to_string()).collect();
        self.encode(&self.claims(subject, &roles, Duration::from_secs(ttl_seconds.max(1))))
    }

    /// 校验 JWT：签名、过期、签发方。
    pub fn validate_token(&self, token: &str) -> Result<Claims, AuthError> {
        let mut validation = Validation::default();
        validation.set_issuer(&[&self.issuer]);
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &validation,
        )
        .map(|data| data.claims)
        .map_err(|_| AuthError::InvalidToken)
    }
}

#[derive(Debug)]
pub enum AuthError {
    IssueFailed,
    InvalidToken,
}

/// 登录请求体。`oidc_code` 为真实 OIDC 授权码；联调环境（dev_mode +
/// 白名单内）也接受 `用户:角色1,角色2` 占位格式。`mfa_code`/`otp` 为可选 OTP。
/// EasySSH 客户端契约（R-20261120-005 集成对齐）：
/// - `token`：直接提交已签发的短期 JWT（官网自助管理台 /admin/issue 产出）→ 校验后重签；
/// - `username`(+`password`)：仅 dev 闸门内放行（等同占位，密码不校验、审计留痕）。
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    pub oidc_code: String,
    #[serde(default)]
    pub mfa_code: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub otp: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // 客户端附带字段；网关按 URL 中的 resource_id 解析后端
    pub resource_id: Option<String>,
}

/// 登录响应：签发的短期 token 与过期秒数。
#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_in: u64,
    pub token_type: &'static str,
}

/// 占位 code 判定：形如 `用户:角色`（含 `:`）。真实 OIDC 授权码不含 `:`。
pub fn is_placeholder_code(code: &str) -> bool {
    code.contains(':')
}

/// 解析占位 code："zhangsan:ops,admin" -> ("zhangsan", ["ops","admin"])。
pub fn parse_placeholder_code(code: &str) -> (String, Vec<String>) {
    match code.split_once(':') {
        Some((sub, roles)) if !sub.is_empty() => (
            sub.to_string(),
            roles
                .split(',')
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        _ => (code.to_string(), vec!["user".to_string()]),
    }
}

/// **占位登录闸门**（R-20261120-006/008）：dev_mode=true 且来源 IP 命中
/// 白名单才放行。ip 解析失败（"unknown"）一律拒绝（fail-closed）。
pub fn placeholder_login_allowed(state: &AppState, ip: &str) -> bool {
    state.dev_mode && crate::cidr::ip_allowed(ip, &state.dev_cidrs)
}

/// POST /api/auth/login 处理。peer 缺失（oneshot 测试 / 无 ConnectInfo）
/// 时来源 IP 记为 "unknown"（闸门 fail-closed）。
async fn login(
    State(state): State<Arc<AppState>>,
    peer: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<LoginRequest>,
) -> Response {
    let ip = peer
        .map(|p| p.0.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let otp_code = req.otp.clone().or_else(|| req.mfa_code.clone());
    let denied_response = |msg: &str, detail: String| -> Response {
        let mut ev = crate::audit::AuditEvent::new("-", None, "auth_placeholder_denied", "-", &ip);
        ev.error = Some(detail);
        state.audit.record(ev);
        (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"code": "auth_placeholder_denied", "message": msg})))
            .into_response()
    };

    // ── EasySSH 客户端 Token 直登：校验已签发 JWT 后重签短期 token ──
    if let Some(tok) = req.token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        match state.auth.validate_token(tok) {
            Ok(old) => {
                let token = match state.auth.issue_token(&old.sub, &old.roles.iter().map(String::as_str).collect::<Vec<_>>()) {
                    Ok(t) => t,
                    Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败").into_response(),
                };
                tracing::info!(user = %old.sub, %ip, "issued short-lived gateway token (direct)");
                state.audit.record(crate::audit::AuditEvent::new(
                    &old.sub,
                    Some(crate::policy::canonical_role(&old.roles).as_str()),
                    "auth_login_direct",
                    "-",
                    &ip,
                ));
                return Json(LoginResponse {
                    token,
                    expires_in: state.auth.default_ttl_seconds(),
                    token_type: "Bearer",
                })
                .into_response();
            }
            Err(_) => {
                let mut ev = crate::audit::AuditEvent::new("-", None, "auth_login_failed", "-", &ip);
                ev.error = Some("direct token invalid or expired".to_string());
                state.audit.record(ev);
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"code": "auth_placeholder_denied", "message": "token 无效或已过期，请在官网自助管理台重新签发"})))
                    .into_response();
            }
        }
    }

    // ── EasySSH 客户端 username/password（inherit/userpass）：仅 dev 闸门内放行 ──
    if let Some(user) = req.username.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        if !placeholder_login_allowed(&state, &ip) {
            tracing::warn!(%ip, dev_mode = state.dev_mode, "username login denied");
            return denied_response(
                "账号密码登录未开放（生产环境请在官网自助管理台签发 Token，或使用 OIDC 授权码）",
                format!("dev_mode={} ip={ip} username_login_gate_denied", state.dev_mode),
            );
        }
        let subject = user.to_string();
        if let Some(resp) = require_otp(&state, &subject, otp_code.as_deref()) {
            return resp;
        }
        let token = match state.auth.issue_token(&subject, &["user"]) {
            Ok(t) => t,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败").into_response(),
        };
        tracing::info!(%subject, %ip, "issued short-lived gateway token (dev username)");
        state.audit.record(crate::audit::AuditEvent::new(
            &subject,
            Some("user"),
            "auth_login_dev_placeholder",
            "-",
            &ip,
        ));
        return Json(LoginResponse {
            token,
            expires_in: state.auth.default_ttl_seconds(),
            token_type: "Bearer",
        })
        .into_response();
    }

    let code = req.oidc_code.trim().to_string();
    if code.is_empty() {
        return (StatusCode::BAD_REQUEST, "oidc_code 不能为空").into_response();
    }

    // ── 占位登录路径（受 dev_mode + CIDR 白名单闸门保护）──
    if is_placeholder_code(&code) {
        if !placeholder_login_allowed(&state, &ip) {
            tracing::warn!(%ip, dev_mode = state.dev_mode, "placeholder login denied");
            return denied_response(
                "占位登录已禁用（dev_mode 关闭或来源不在白名单）",
                format!("dev_mode={} ip={ip} cidr_allowlist_miss_or_disabled", state.dev_mode),
            );
        }
        let (subject, roles) = parse_placeholder_code(&code);
        // OTP 二次认证（本地 totp_secret 用户）
        if let Some(resp) = require_otp(&state, &subject, otp_code.as_deref()) {
            return resp;
        }
        let token = match state.auth.issue_token(&subject, &roles.iter().map(String::as_str).collect::<Vec<_>>()) {
            Ok(t) => t,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败").into_response(),
        };
        tracing::info!(%subject, %ip, "issued short-lived gateway token (placeholder/dev)");
        let mut ev = crate::audit::AuditEvent::new(
            &subject,
            Some(crate::policy::canonical_role(&roles).as_str()),
            "auth_login_dev_placeholder",
            "-",
            &ip,
        );
        ev.error = None;
        state.audit.record(ev);
        return Json(LoginResponse {
            token,
            expires_in: state.auth.default_ttl_seconds(),
            token_type: "Bearer",
        })
        .into_response();
    }

    // ── 真实 OIDC code 主路径（不受占位闸门影响）──
    let Some(oidc) = state.oidc.as_ref() else {
        let mut ev = crate::audit::AuditEvent::new("-", None, "auth_login_failed", "-", &ip);
        ev.error = Some("oidc_not_configured".to_string());
        state.audit.record(ev);
        return (StatusCode::BAD_REQUEST, "网关未配置 [oidc]，占位登录已禁用").into_response();
    };
    let token_json = match do_real_oidc_exchange(oidc, &code).await {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(%ip, %err, "oidc code exchange failed");
            let mut ev = crate::audit::AuditEvent::new("-", None, "auth_login_failed", "-", &ip);
            ev.error = Some(err.clone());
            state.audit.record(ev);
            return (StatusCode::UNAUTHORIZED, format!("OIDC 换票失败: {err}")).into_response();
        }
    };
    let sub = token_json
        .get("sub")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    if sub.is_empty() {
        let mut ev = crate::audit::AuditEvent::new("-", None, "auth_login_failed", "-", &ip);
        ev.error = Some("id_token_missing_sub".to_string());
        state.audit.record(ev);
        return (StatusCode::UNAUTHORIZED, "id_token 缺 sub").into_response();
    }
    let roles = extract_roles(&token_json, oidc.cfg.effective_roles_claim());
    if let Some(resp) = require_otp(&state, &sub, otp_code.as_deref()) {
        return resp;
    }
    let token = match state.auth.issue_token(&sub, &roles.iter().map(String::as_str).collect::<Vec<_>>()) {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "签发 token 失败").into_response(),
    };
    tracing::info!(%sub, %ip, "issued short-lived gateway token (oidc)");
    state.audit.record(crate::audit::AuditEvent::new(
        &sub,
        Some(crate::policy::canonical_role(&roles).as_str()),
        "auth_login",
        "-",
        &ip,
    ));
    Json(LoginResponse {
        token,
        expires_in: state.auth.default_ttl_seconds(),
        token_type: "Bearer",
    })
    .into_response()
}

/// 本地 OTP 检查：配置了 totp_secret 的用户必须携带有效 6 位 TOTP
/// （30s 步长 ±1 窗口）。缺失/错误返回 401 "otp_required"（Some(resp)）。
fn require_otp(state: &AppState, subject: &str, mfa_code: Option<&str>) -> Option<Response> {
    let secret = state.totp_secrets.get(subject)?;
    let code = mfa_code.map(str::trim).unwrap_or("");
    let ok = !code.is_empty() && verify_totp(secret, code, 1, SystemTime::now());
    if ok {
        return None;
    }
    tracing::info!(%subject, "otp required/invalid for user");
    // 客户端契约：JSON {"code":"otp_required"}（EasySSH 客户端据此弹 6 位码重连）
    Some((
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"code": "otp_required"})),
    )
        .into_response())
}

/// 真实 OIDC：discovery → token 端点换票 → id_token 签名校验 → claims JSON。
pub async fn do_real_oidc_exchange(
    oidc: &OidcState,
    code: &str,
) -> Result<serde_json::Value, String> {
    let doc = oidc.discovery().await?;
    let form = [
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), oidc.cfg.redirect_uri.clone().unwrap_or_default()),
        ("client_id".to_string(), oidc.cfg.client_id.clone().unwrap_or_default()),
        ("client_secret".to_string(), oidc.cfg.client_secret.clone().unwrap_or_default()),
    ];
    let resp = oidc.fetcher.exchange_code(&doc.token_endpoint, &form).await?;
    let id_token = resp
        .get("id_token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "token 响应缺 id_token".to_string())?
        .to_string();
    validate_id_token(oidc, &id_token).await
}

/// auth_guard：校验 `Authorization: Bearer <token>`，将 Claims 注入请求扩展。
pub async fn auth_guard(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let token = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim);

    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "缺少 Bearer token").into_response();
    };
    match state.auth.validate_token(token) {
        Ok(claims) => {
            req.extensions_mut().insert(claims);
            next.run(req).await
        }
        Err(AuthError::InvalidToken) => (StatusCode::UNAUTHORIZED, "token 无效或已过期").into_response(),
        Err(AuthError::IssueFailed) => (StatusCode::INTERNAL_SERVER_ERROR, "内部认证错误").into_response(),
    }
}

/// 是否拥有 admin 角色。
pub fn is_admin(claims: &Claims) -> bool {
    claims.roles.iter().any(|r| r == "admin")
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/auth/login", post(login))
        .with_state(state)
}

// ---------------------------------------------------------------------
// TOTP（RFC 6238）手写实现：Base32 + HMAC-SHA1 + 30s 步长 ±1 窗口容差
// ---------------------------------------------------------------------

/// TOTP 步长（RFC 6238 默认 30 秒）。
pub const TOTP_STEP: u64 = 30;
/// 默认 6 位数字码长度。
pub const TOTP_DIGITS: u32 = 6;

/// RFC 4648 Base32 解码（A-Z/2-7；容忍小写、空格、`-` 与 `=` 填充）。
pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = Vec::new();
    let mut bits: u32 = 0;
    let mut acc: u32 = 0;
    for ch in input.chars() {
        if ch == '=' || ch == ' ' || ch == '-' {
            continue;
        }
        let upper = ch.to_ascii_uppercase();
        let idx = ALPHABET.iter().position(|&a| a == upper as u8)? as u32;
        acc = (acc << 5) | idx;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// HOTP 动态截断（RFC 4226 5.3）：HMAC-SHA1 20 字节 -> 6 位十进制码。
fn hotp_code(key: &[u8], counter: u64) -> String {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0F) as usize;
    let bin = u32::from_be_bytes([
        digest[offset] & 0x7F,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:0width$}", bin % 10u32.pow(TOTP_DIGITS), width = TOTP_DIGITS as usize)
}

/// TOTP：给定 Base32 密钥与 Unix 秒时刻，计算该 30s 步长的 6 位码。
#[allow(dead_code)] // 客户端侧生成验证码用；网关侧仅测试直接消费
pub fn totp_code_at(secret_base32: &str, unix_seconds: u64) -> Option<String> {
    let key = base32_decode(secret_base32)?;
    Some(hotp_code(&key, unix_seconds / TOTP_STEP))
}

/// TOTP 校验：当前时刻及 ±`tolerance` 个窗口内任一码匹配即通过。
pub fn verify_totp(secret_base32: &str, code: &str, tolerance: u64, now: SystemTime) -> bool {
    let Some(key) = base32_decode(secret_base32) else {
        return false;
    };
    let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let current = now_secs / TOTP_STEP;
    let code = code.trim();
    (0..=tolerance)
        .flat_map(|d| [current.wrapping_sub(d), current.wrapping_add(d)])
        .any(|step| hotp_code(&key, step) == code)
}

// ---------------------------------------------------------------------
// OIDC 真实对接：discovery / JWKS 缓存 + id_token 校验
// ---------------------------------------------------------------------

/// JWKS 缓存有效期（秒）。
pub const JWKS_CACHE_TTL: u64 = 600;
/// discovery 文档缓存有效期（秒）。
pub const DISCOVERY_CACHE_TTL: u64 = 3600;

/// IdP HTTP 动作抽象（对象安全，供单测注入 mock）。
pub trait TokenFetcher: Send + Sync {
    /// POST token 端点（表单编码），返回原始 JSON。
    fn exchange_code<'a>(
        &'a self,
        token_endpoint: &'a str,
        form: &'a [(String, String)],
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send + 'a>>;
    /// GET 任意 URL（discovery / JWKS），返回响应体文本。
    fn fetch_text<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;
}

/// 生产实现：reqwest + rustls（webpki-roots）。
pub struct HttpIdpClient {
    client: reqwest::Client,
}

impl HttpIdpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest client build"),
        }
    }
}

impl Default for HttpIdpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenFetcher for HttpIdpClient {
    fn exchange_code<'a>(
        &'a self,
        token_endpoint: &'a str,
        form: &'a [(String, String)],
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let resp = self
                .client
                .post(token_endpoint)
                .form(form)
                .send()
                .await
                .map_err(|e| format!("token 端点请求失败: {e}"))?;
            let status = resp.status();
            let body = resp.text().await.map_err(|e| format!("token 响应读取失败: {e}"))?;
            if !status.is_success() {
                return Err(format!("token 端点返回 {status}"));
            }
            serde_json::from_str(&body).map_err(|e| format!("token 响应非 JSON: {e}"))
        })
    }

    fn fetch_text<'a>(
        &'a self,
        url: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| format!("请求 {url} 失败: {e}"))?;
            let status = resp.status();
            let body = resp.text().await.map_err(|e| format!("响应读取失败: {e}"))?;
            if !status.is_success() {
                return Err(format!("{url} 返回 {status}"));
            }
            Ok(body)
        })
    }
}

/// OIDC discovery 文档（MVP 只取 4 个必需字段）。
#[derive(Debug, Clone)]
pub struct DiscoveryDoc {
    pub issuer: String,
    #[allow(dead_code)]
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

impl DiscoveryDoc {
    fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let get = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("discovery 文档缺 {k}"))
        };
        Ok(Self {
            issuer: get("issuer")?,
            authorization_endpoint: get("authorization_endpoint")?,
            token_endpoint: get("token_endpoint")?,
            jwks_uri: get("jwks_uri")?,
        })
    }
}

/// OIDC 运行态：配置 + discovery/JWKS 缓存。
pub struct OidcState {
    pub cfg: crate::config::OidcConfig,
    fetcher: Arc<dyn TokenFetcher>,
    discovery: tokio::sync::RwLock<Option<(DiscoveryDoc, Instant)>>,
    jwks: tokio::sync::RwLock<Option<(String, Instant)>>,
}

impl OidcState {
    pub fn new(cfg: crate::config::OidcConfig, fetcher: Arc<dyn TokenFetcher>) -> Self {
        Self {
            cfg,
            fetcher,
            discovery: tokio::sync::RwLock::new(None),
            jwks: tokio::sync::RwLock::new(None),
        }
    }

    /// discovery 文档（缓存 1h）。URL：显式 discovery_url 优先，否则
    /// `{issuer 去尾斜杠}/.well-known/openid-configuration`。issuer 精确匹配。
    pub async fn discovery(&self) -> Result<DiscoveryDoc, String> {
        {
            let cached = self.discovery.read().await;
            if let Some((doc, at)) = cached.as_ref() {
                if at.elapsed().as_secs() < DISCOVERY_CACHE_TTL {
                    return Ok(doc.clone());
                }
            }
        }
        let url = self
            .cfg
            .discovery_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "{}/.well-known/openid-configuration",
                    self.cfg.issuer.as_deref().unwrap_or("").trim_end_matches('/')
                )
            });
        let text = self.fetcher.fetch_text(&url).await?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("discovery 响应非 JSON: {e}"))?;
        let doc = DiscoveryDoc::from_json(&value)?;
        let cfg_issuer = self.cfg.issuer.as_deref().unwrap_or("").trim_end_matches('/');
        let doc_issuer = doc.issuer.trim_end_matches('/');
        if !cfg_issuer.is_empty() && doc_issuer != cfg_issuer {
            return Err(format!("discovery issuer 不匹配: {doc_issuer:?} != {cfg_issuer:?}"));
        }
        *self.discovery.write().await = Some((doc.clone(), Instant::now()));
        Ok(doc)
    }

    /// JWKS 原文（缓存 10min）。
    pub async fn jwks_raw(&self) -> Result<String, String> {
        {
            let cached = self.jwks.read().await;
            if let Some((raw, at)) = cached.as_ref() {
                if at.elapsed().as_secs() < JWKS_CACHE_TTL {
                    return Ok(raw.clone());
                }
            }
        }
        let doc = self.discovery().await?;
        let raw = self.fetcher.fetch_text(&doc.jwks_uri).await?;
        *self.jwks.write().await = Some((raw.clone(), Instant::now()));
        Ok(raw)
    }
}

/// 从 claims JSON 中按配置的 roles_claim 提取角色列表。
/// 顺序：roles_claim（默认 "roles"）→ "groups" → ["user"]。
pub fn extract_roles(claims: &serde_json::Value, roles_claim: &str) -> Vec<String> {
    fn as_string_list(v: &serde_json::Value) -> Option<Vec<String>> {
        match v {
            serde_json::Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .filter(|s| !s.trim().is_empty())
                    .collect(),
            ),
            serde_json::Value::String(s) if !s.trim().is_empty() => Some(
                s.split(|c: char| c == ',' || c == ' ')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            ),
            _ => None,
        }
    }
    for key in [roles_claim, "groups"] {
        if let Some(list) = claims.get(key).and_then(as_string_list) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    vec!["user".to_string()]
}

/// 校验 id_token：签名（JWKS kid 匹配）+ iss/aud/exp。返回完整 claims JSON。
pub async fn validate_id_token(
    oidc: &OidcState,
    id_token: &str,
) -> Result<serde_json::Value, String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err("id_token 格式非法（应为 3 段 JWT）".into());
    }
    let header_json = base64url_decode_json(parts[0])?;
    let kid = header_json
        .get("kid")
        .and_then(|k| k.as_str())
        .ok_or_else(|| "id_token header 缺 kid（本网关要求 JWKS kid 匹配）".to_string())?
        .to_string();
    let alg = header_json.get("alg").and_then(|a| a.as_str()).unwrap_or("");
    let algorithm = match alg {
        "RS256" => Algorithm::RS256,
        "ES256" => Algorithm::ES256,
        other => return Err(format!("不支持的 id_token 算法: {other:?}")),
    };

    let jwks_raw = oidc.jwks_raw().await?;
    let jwk_set: jsonwebtoken::jwk::JwkSet =
        serde_json::from_str(&jwks_raw).map_err(|e| format!("JWKS 解析失败: {e}"))?;
    let jwk = jwk_set
        .find(&kid)
        .ok_or_else(|| format!("JWKS 中无 kid={kid:?} 的密钥（拒绝）"))?;
    let decoding = DecodingKey::from_jwk(jwk).map_err(|e| format!("JWK 构建失败: {e}"))?;

    let mut validation = Validation::new(algorithm);
    validation.set_issuer(&[oidc.cfg.issuer.as_deref().unwrap_or("")]);
    validation.set_audience(&[oidc.cfg.client_id.as_deref().unwrap_or("")]);
    validation.leeway = 60;
    validation.validate_aud = true;

    decode::<serde_json::Value>(id_token, &decoding, &validation)
        .map(|data| data.claims)
        .map_err(|e| format!("id_token 校验失败: {e}"))
}

/// base64url 解码（无填充容忍）→ UTF-8 JSON。
fn base64url_decode_json(seg: &str) -> Result<serde_json::Value, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg.trim_end_matches('='))
        .map_err(|e| format!("JWT 段 base64url 解码失败: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("JWT 段非 JSON: {e}"))
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AuthState {
        AuthState::new("test-secret", DEFAULT_TOKEN_TTL)
    }

    #[test]
    fn issue_token_contains_subject_and_roles() {
        let s = state();
        let token = s.issue_token("alice", &["ops", "admin"]).expect("issue");
        let claims = s.validate_token(&token).expect("validate");
        assert_eq!(claims.sub, "alice");
        assert_eq!(claims.roles, vec!["ops".to_string(), "admin".to_string()]);
        assert_eq!(claims.iss, "easyssh-gateway");
        assert_eq!(claims.exp - claims.iat, DEFAULT_TOKEN_TTL, "短期语义 600s");
    }

    #[test]
    fn wrong_secret_and_expired_tokens_rejected() {
        let s = state();
        let token = s.issue_token("bob", &["user"]).expect("issue");
        let other = AuthState::new("wrong-secret", DEFAULT_TOKEN_TTL);
        assert!(matches!(other.validate_token(&token), Err(AuthError::InvalidToken)));
        // 手工编码过期 10 分钟的 claims（默认 leeway 60s，确定性过期）
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = Claims {
            sub: "carol".into(),
            roles: vec!["user".into()],
            iat: now - 1_200,
            exp: now - 600,
            iss: "easyssh-gateway".into(),
        };
        let expired = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap();
        assert!(matches!(s.validate_token(&expired), Err(AuthError::InvalidToken)));
    }

    #[test]
    fn placeholder_detection_and_parse() {
        assert!(is_placeholder_code("用户:admin"));
        assert!(is_placeholder_code("zhangsan:ops,admin"));
        assert!(!is_placeholder_code("real-oidc-authorization-code-xyz"));
        let (sub, roles) = parse_placeholder_code("zhangsan:ops, admin");
        assert_eq!(sub, "zhangsan");
        assert_eq!(roles, vec!["ops".to_string(), "admin".to_string()]);
        let (sub, roles) = parse_placeholder_code("onlysub");
        assert_eq!((sub.as_str(), roles.as_slice()), ("onlysub", ["user".to_string()].as_slice()));
    }

    // ---------------- TOTP ----------------

    /// RFC 6238 附录 B 参考向量（密钥 "12345678901234567890" 的 Base32）：
    /// T=59 → 6 位截取 287082。
    #[test]
    fn totp_matches_rfc6238_reference_vector() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let code = totp_code_at(secret, 59).expect("decode");
        assert_eq!(code, "287082");
    }

    #[test]
    fn base32_decode_variants() {
        assert_eq!(base32_decode("JBSWY3DP"), Some(b"Hello".to_vec()));
        assert_eq!(base32_decode("jbswy3dp"), base32_decode("JBSWY3DP"));
        assert_eq!(base32_decode("JBSWY3DP==="), base32_decode("JBSWY3DP"));
        assert_eq!(base32_decode("JBSWY3DP1"), None);
    }

    #[test]
    fn verify_totp_window_tolerance_and_reject() {
        let secret = "JBSWY3DPEHPK3PXP";
        let t0 = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let code = totp_code_at(secret, 1_000_000).unwrap();
        assert!(verify_totp(secret, &code, 1, t0));
        assert!(verify_totp(secret, &totp_code_at(secret, 1_000_000 - TOTP_STEP).unwrap(), 1, t0));
        assert!(verify_totp(secret, &totp_code_at(secret, 1_000_000 + TOTP_STEP).unwrap(), 1, t0));
        assert!(!verify_totp(secret, &totp_code_at(secret, 1_000_000 - 2 * TOTP_STEP).unwrap(), 1, t0));
        assert!(!verify_totp(secret, "", 1, t0));
        assert!(!verify_totp("NOT@VALID32!", &code, 1, t0));
    }
}
