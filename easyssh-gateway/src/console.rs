//! Web 控制台：网关内置单页控制台。
//!
//! 路由（全部复用网关自身 axum，无独立前端工程；HTML 单文件内联 CSS）：
//! - GET  `/` `/console`            登录页（dev_mode 时显示占位登录表单）
//! - POST `/console/login`          占位登录（CSRF + dev_mode 闸门 + 可选 TOTP）
//! - GET  `/console/dashboard`      控制台（资源列表 / 策略矩阵只读 / 审计检索）
//! - POST `/console/logout`         登出（CSRF + 审计）
//!
//! 会话：JWT（30min 滑动过期，HttpOnly cookie `ezs_session`，SameSite=Lax）。
//! CSRF：double-submit cookie（`ezs_csrf` 与表单 `csrf_token` 必须一致），
//! 所有 POST 强制校验。控制台占位登录与 /api/auth/login 同一 dev_mode 闸门
//! （R-20261120-006）：dev_mode=false 时表单不渲染且提交直接拒绝。

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{auth::Claims, AppState};

/// 来源 IP：优先 X-Real-IP / X-Forwarded-For，缺失回退 "unknown"。
pub fn client_ip_from_headers(headers: &HeaderMap) -> String {
    for name in ["x-real-ip", "x-forwarded-for"] {
        if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
            let first = v.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "unknown".to_string()
}

/// 控制台会话 cookie 名。
pub const SESSION_COOKIE: &str = "ezs_session";
/// CSRF double-submit cookie 名。
pub const CSRF_COOKIE: &str = "ezs_csrf";
/// 控制台会话 TTL（秒）：30 分钟滑动过期。
pub const CONSOLE_SESSION_TTL: u64 = 1800;
/// 滑动续签阈值：剩余寿命低于 TTL/2 时滚动 Set-Cookie。
const SLIDE_THRESHOLD: u64 = CONSOLE_SESSION_TTL / 2;

// ---------------------------------------------------------------------
// cookie / CSRF 基础设施
// ---------------------------------------------------------------------

/// 新建 CSRF token 与 Set-Cookie 头（double-submit 用）。
pub fn new_csrf_cookie() -> (String, String) {
    let token = uuid::Uuid::new_v4().to_string();
    let cookie = format!("{CSRF_COOKIE}={token}; Path=/; SameSite=Lax; Max-Age=1800");
    (token, cookie)
}

/// 校验 double-submit CSRF：cookie 与表单 token 必须同时存在且相等（恒定时间）。
pub fn csrf_ok(headers: &HeaderMap, form_token: &str) -> bool {
    let cookie_token = cookie_value(headers, CSRF_COOKIE).unwrap_or_default();
    if form_token.is_empty() || cookie_token.is_empty() {
        return false;
    }
    constant_time_eq(cookie_token.as_bytes(), form_token.as_bytes())
}

/// 从 Cookie 头取指定 cookie 值。
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// 会话 Set-Cookie（HttpOnly + SameSite=Lax + 30min Max-Age）。
pub fn session_cookie(token: &str) -> String {
    format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={CONSOLE_SESSION_TTL}"
    )
}

/// 登录成功响应：302 /console/dashboard + 会话 cookie。
pub fn session_redirect(token: &str) -> Response {
    (
        StatusCode::FOUND,
        [("Location", "/console/dashboard".to_string()), ("Set-Cookie", session_cookie(token))],
    )
        .into_response()
}

/// 登出响应：302 / + 清除 cookie。
fn logout_response() -> Response {
    (
        StatusCode::FOUND,
        [("Location", "/".to_string())],
        [(
            "Set-Cookie",
            format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
        )],
    )
        .into_response()
}

/// 会话中间件语义：校验 + 滑动续签（剩余 < 阈值时补 Set-Cookie）。
fn session_with_slide(state: &AppState, headers: &HeaderMap) -> (Option<Claims>, Option<String>) {
    let Some(token) = cookie_value(headers, SESSION_COOKIE) else {
        return (None, None);
    };
    let Ok(claims) = state.auth.validate_token(&token) else {
        return (None, None);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut slide = None;
    if claims.exp.saturating_sub(now) < SLIDE_THRESHOLD {
        if let Ok(new_token) = state.auth.issue_token_with_ttl(
            &claims.sub,
            &claims.roles.iter().map(String::as_str).collect::<Vec<_>>(),
            CONSOLE_SESSION_TTL,
        ) {
            slide = Some(session_cookie(&new_token));
        }
    }
    (Some(claims), slide)
}

/// 恒定时间比较（防时序侧信道）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------
// HTML 渲染（单文件 + 内联 CSS 深色简版）
// ---------------------------------------------------------------------

/// HTML 转义（防 XSS：审计字段/用户名等全部来自外部输入）。
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const STYLE: &str = r#"
body{background:#12161c;color:#d7dce3;font-family:system-ui,"Segoe UI",sans-serif;margin:0}
.wrap{max-width:1100px;margin:40px auto;padding:0 16px}
.card{background:#1b212b;border:1px solid #2a323e;border-radius:8px;padding:16px 20px;margin-bottom:16px}
h1{font-size:20px;margin:8px 0 16px}h2{font-size:15px;color:#9fb0c3;margin:0 0 10px}
table{width:100%;border-collapse:collapse;font-size:13px}
th,td{padding:6px 8px;border-bottom:1px solid #2a323e;text-align:left}
th{color:#9fb0c3;font-weight:600}
button,input,select{background:#232b37;color:#d7dce3;border:1px solid #3a4655;border-radius:4px;padding:6px 10px;font-size:13px}
button{cursor:pointer}
a{color:#6ea8fe;text-decoration:none}
.ok{color:#7ec699}.deny{color:#e07a7a}
.muted{color:#7a8798;font-size:12px}
.err{color:#e07a7a;margin-top:10px}
nav{display:flex;gap:14px;margin-bottom:16px;font-size:14px}
.badge{display:inline-block;background:#232b37;border:1px solid #3a4655;border-radius:10px;padding:1px 8px;font-size:12px}
"#;

fn html_page(title: &str, body: String) -> Response {
    let html = format!(
        "<!DOCTYPE html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{}</title><style>{}</style></head><body><div class=\"wrap\">{}</div></body></html>",
        esc(title),
        STYLE,
        body
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

/// 登录页。占位登录表单仅在 dev_mode=true 时渲染（生产 dev_mode=false
/// 时显示"占位登录已禁用"提示，正式凭据走客户端 API /admin/issue 签发）。
fn page_login(state: &AppState, csrf: &str, csrf_cookie: &str, error: Option<&str>) -> Response {
    let placeholder_form = if state.dev_mode {
        format!(
            "<form method=\"post\" action=\"/console/login\">\
<input type=\"hidden\" name=\"csrf_token\" value=\"{csrf}\">\
<p>占位登录（dev_mode，仅白名单内来源）</p>\
<p><input name=\"oidc_code\" placeholder=\"user:roles（如 zhangsan:user）\"></p>\
<p><input name=\"mfa_code\" placeholder=\"OTP（如启用）\"></p>\
<p><button type=\"submit\">登录</button></p></form>"
        )
    } else {
        "<p class=\"muted\">占位登录已禁用（dev_mode=false，生产口径）。\
请通过 EasySSH 客户端登录获取网关 token。</p>".to_string()
    };
    let err_html = error.map(|e| format!("<p class=\"err\">{}</p>", esc(e))).unwrap_or_default();
    let body = format!(
        "<h1>EasySSH 网关控制台</h1><div class=\"card\">{placeholder_form}{err_html}</div>\
<p class=\"muted\">EasySSH Gateway · v5.2.3</p>"
    );
    let mut resp = html_page("EasySSH 控制台登录", body);
    resp.headers_mut()
        .append(header::SET_COOKIE, HeaderValue::from_str(csrf_cookie).expect("csrf cookie"));
    resp
}

/// 审计表 HTML（过滤 + 分页后的行）。
fn audit_table_html(rows: &[crate::audit::AuditEvent]) -> String {
    if rows.is_empty() {
        return "<p class=\"muted\">（无匹配审计记录）</p>".to_string();
    }
    let mut t = String::from(
        "<table><tr><th>时间</th><th>用户</th><th>角色</th><th>事件</th><th>资源</th>\
<th>来源 IP</th><th>结果</th></tr>",
    );
    for e in rows {
        let result = e.result();
        let cls = if result == "ok" { "ok" } else { "deny" };
        t.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
<td class=\"{cls}\">{}</td></tr>",
            e.timestamp,
            esc(&e.user),
            esc(e.role.as_deref().unwrap_or("-")),
            esc(&e.event),
            esc(&e.resource),
            esc(&e.source_ip),
            esc(&result),
        ));
    }
    t.push_str("</table>");
    t
}

/// 控制台主页面。
fn page_dashboard(
    state: &AppState,
    claims: &Claims,
    q: &DashQuery,
    slide_cookie: Option<String>,
) -> Response {
    let is_admin = crate::auth::is_admin(claims);
    let mut body = String::from("<h1>EasySSH 控制台</h1><nav>");
    body.push_str(&format!(
        "<a href=\"/console/dashboard\">概览</a> · <span class=\"badge\">{}</span>",
        esc(&claims.sub)
    ));
    body.push_str(&format!(
        "<form method=\"post\" action=\"/console/logout\" style=\"margin-left:auto\">\
<input type=\"hidden\" name=\"csrf_token\" value=\"{}\"><button type=\"submit\">登出</button></form></nav>",
        esc(&q.csrf_for_nav.clone().unwrap_or_default())
    ));

    // 资源列表（所有已登录角色可见）
    body.push_str("<div class=\"card\"><h2>资源列表</h2><table><tr><th>ID</th><th>名称</th></tr>");
    let resources = state.resources.list();
    if resources.is_empty() {
        body.push_str("<tr><td colspan=\"2\" class=\"muted\">（无注册资源）</td></tr>");
    }
    for r in &resources {
        body.push_str(&format!("<tr><td>{}</td><td>{}</td></tr>", esc(&r.id), esc(&r.name)));
    }
    body.push_str("</table></div>");

    // 策略矩阵只读视图（仅 admin）
    if is_admin {
        body.push_str("<div class=\"card\"><h2>策略矩阵（只读）</h2><table>\
<tr><th>资源</th><th>角色</th><th>动作</th><th>结论</th></tr>");
        let rules = state.policy.describe();
        if rules.is_empty() {
            body.push_str("<tr><td colspan=\"4\" class=\"muted\">（无显式规则：默认矩阵 deny by default、admin 全通）</td></tr>");
        }
        for (res, role, action, allow) in &rules {
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"{}\">{}</td></tr>",
                esc(res),
                esc(role),
                esc(action),
                if *allow { "ok" } else { "deny" },
                if *allow { "Allow" } else { "Deny" },
            ));
        }
        body.push_str("</table></div>");

        // 审计检索表（仅 admin）
        let audit_q = crate::audit::AuditQuery {
            user: q.user.clone().filter(|s| !s.is_empty()),
            event: None,
            resource: q.resource.clone().filter(|s| !s.is_empty()),
            result: q.result.clone().filter(|s| !s.is_empty()),
            limit: None,
        };
        let all: Vec<_> = state
            .audit
            .snapshot()
            .into_iter()
            .filter(|e| audit_q.matches(e))
            .collect();
        let total = all.len();
        let page_size = 20usize.max(q.page_size.unwrap_or(20).clamp(5, 100));
        let pages = total.div_ceil(page_size).max(1);
        let page = q.page.unwrap_or(1).clamp(1, pages);
        let start = (page - 1) * page_size;
        let end = (start + page_size).min(total);
        let slice: Vec<_> = all[start..end].to_vec();
        body.push_str(&format!(
            "<div class=\"card\"><h2>审计检索（共 {total} 条）</h2>\
<form method=\"get\" action=\"/console/dashboard\">\
用户 <input name=\"user\" value=\"{}\"> \
资源 <input name=\"resource\" value=\"{}\"> \
<button type=\"submit\">筛选</button></form><br>",
            esc(q.user.as_deref().unwrap_or("")),
            esc(q.resource.as_deref().unwrap_or("")),
        ));
        body.push_str(&audit_table_html(&slice));
        body.push_str(&format!("<p class=\"muted\">第 {page}/{pages} 页</p></div>"));
    }

    // OTP 绑定状态（当前用户）
    let otp_bound = state.totp_secrets.contains_key(&claims.sub);
    body.push_str(&format!(
        "<div class=\"card\"><h2>OTP 绑定状态</h2><p>{}</p></div>",
        if otp_bound {
            "<span class=\"ok\">已绑定本地 TOTP（登录要求 OTP 二次认证）</span>"
        } else {
            "<span class=\"muted\">未绑定本地 TOTP</span>"
        }
    ));

    let mut resp = html_page("EasySSH 控制台", body);
    if let Some(cookie) = slide_cookie {
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    resp
}

/// GET / 、GET /console —— 登录页；已登录则 302 到 dashboard。
async fn login_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (claims, _) = session_with_slide(&state, &headers);
    if claims.is_some() {
        return axum::response::Redirect::to("/console/dashboard").into_response();
    }
    let (csrf, cookie) = new_csrf_cookie();
    page_login(&state, &csrf, &cookie, None)
}

/// GET /console/dashboard 查询参数。
#[derive(Debug, Deserialize, Default)]
pub struct DashQuery {
    pub user: Option<String>,
    pub resource: Option<String>,
    pub result: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    /// 导航表单所需的 CSRF（来自请求 cookie 回填）。
    #[serde(default)]
    pub csrf_for_nav: Option<String>,
}

async fn dashboard(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<DashQuery>,
    headers: HeaderMap,
) -> Response {
    let (claims, slide) = session_with_slide(&state, &headers);
    let Some(claims) = claims else {
        return axum::response::Redirect::to("/").into_response();
    };
    let mut q = q;
    q.csrf_for_nav = cookie_value(&headers, CSRF_COOKIE);
    page_dashboard(&state, &claims, &q, slide)
}

/// POST /console/login 表单（csrf_token 允许缺失以便统一走 CSRF 校验拒绝）。
#[derive(Debug, Deserialize)]
pub struct ConsoleLoginForm {
    pub csrf_token: Option<String>,
    pub oidc_code: String,
    #[serde(default)]
    pub mfa_code: Option<String>,
}

/// POST /console/login —— 占位登录（CSRF 强制；与 API 登录同一 dev_mode 闸门）。
async fn console_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<ConsoleLoginForm>,
) -> Response {
    let ip = client_ip_from_headers(&headers);
    if !csrf_ok(&headers, form.csrf_token.as_deref().unwrap_or("")) {
        return (StatusCode::FORBIDDEN, "CSRF token 缺失或不匹配").into_response();
    }
    // dev_mode 闸门（R-20261120-006，与 /api/auth/login 同口径）
    if !crate::auth::placeholder_login_allowed(&state, &ip) {
        let mut ev = crate::audit::AuditEvent::new("-", None, "auth_placeholder_denied", "-", &ip);
        ev.error = Some(format!("console dev_mode={} ip={ip}", state.dev_mode));
        state.audit.record(ev);
        let (csrf, cookie) = new_csrf_cookie();
        return page_login(&state, &csrf, &cookie, Some("占位登录已禁用（dev_mode 关闭或来源不在白名单）"));
    }
    let (subject, roles) = crate::auth::parse_placeholder_code(&form.oidc_code);
    // TOTP 不变量
    if let Some(secret) = state.totp_secrets.get(&subject) {
        let code = form.mfa_code.as_deref().map(str::trim).unwrap_or("");
        let ok = !code.is_empty()
            && crate::auth::verify_totp(secret, code, 1, std::time::SystemTime::now());
        if !ok {
            let mut ev =
                crate::audit::AuditEvent::new(&subject, None, "console_login_failed", "-", &ip);
            ev.error = Some("otp_required_or_invalid".into());
            state.audit.record(ev);
            let (csrf, cookie) = new_csrf_cookie();
            return page_login(&state, &csrf, &cookie, Some("需要有效的 OTP 验证码"));
        }
    }
    let token = match state.auth.issue_token_with_ttl(
        &subject,
        &roles.iter().map(String::as_str).collect::<Vec<_>>(),
        CONSOLE_SESSION_TTL,
    ) {
        Ok(t) => t,
        Err(err) => {
            let _ = err;
            return (StatusCode::INTERNAL_SERVER_ERROR, "签发会话失败").into_response();
        }
    };
    state.audit.record(crate::audit::AuditEvent::new(
        &subject,
        Some(crate::policy::canonical_role(&roles).as_str()),
        "console_login",
        "-",
        &ip,
    ));
    session_redirect(&token)
}

/// POST /console/logout 表单。
#[derive(Debug, Deserialize)]
pub struct LogoutForm {
    pub csrf_token: Option<String>,
}

async fn console_logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<LogoutForm>,
) -> Response {
    let ip = client_ip_from_headers(&headers);
    if !csrf_ok(&headers, form.csrf_token.as_deref().unwrap_or("")) {
        return (StatusCode::FORBIDDEN, "CSRF token 缺失或不匹配").into_response();
    }
    let (claims, _) = session_with_slide(&state, &headers);
    if let Some(c) = claims {
        state.audit.record(crate::audit::AuditEvent::new(
            &c.sub,
            Some(crate::policy::canonical_role(&c.roles).as_str()),
            "console_logout",
            "-",
            &ip,
        ));
    }
    logout_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(login_page))
        .route("/console", get(login_page))
        .route("/console/dashboard", get(dashboard))
        .route("/console/login", post(console_login))
        .route("/console/logout", post(console_logout))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthState;
    use crate::resources::ResourceRegistry;
    use axum::body::Body;
    use axum::http::{Request, StatusCode as Code};
    use tower::ServiceExt;

    fn test_state(dev_mode: bool) -> Arc<AppState> {
        let pc = crate::policy::PolicyConfig {
            rules: vec![crate::policy::PolicyRuleConfig {
                resource: "*".into(),
                role: "user".into(),
                action: "connect".into(),
                decision: "allow".into(),
            }],
        };
        Arc::new(AppState {
            auth: AuthState::new("test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::from_configs(&[crate::resources::ResourceConfig {
                id: "ssh-jump01".into(),
                name: "Linux 跳板".into(),
                host: "10.0.0.5".into(),
                port: 22,
                allowed_roles: None,
            }]),
            admin_key: None,
            policy: crate::policy::PolicyHandle::from_matrix(
                crate::policy::PolicyMatrix::from_config(&pc).expect("matrix"),
            ),
            totp_secrets: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("otpuser".to_string(), "JBSWY3DPEHPK3PXP".to_string());
                m
            },
            oidc: None,
            dev_mode,
            dev_cidrs: std::sync::Arc::new(crate::cidr::loopback_defaults()),
        })
    }

    fn console_app(state: &Arc<AppState>) -> axum::Router {
        router(Arc::clone(state)).with_state(Arc::clone(state))
    }

    #[tokio::test]
    async fn login_page_hides_placeholder_form_when_dev_mode_off() {
        let state = test_state(false);
        let resp = console_app(&state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), Code::OK);
        let cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&body).to_string();
        assert!(body.contains("占位登录已禁用"));
        assert!(!body.contains("/console/login\""), "dev_mode=false 不渲染登录表单");
        assert!(cookie.starts_with(&format!("{CSRF_COOKIE}=")));
    }

    #[tokio::test]
    async fn login_page_shows_placeholder_form_when_dev_mode_on() {
        let state = test_state(true);
        let resp = console_app(&state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&body).to_string();
        assert!(body.contains("/console/login"));
        assert!(body.contains("dev_mode"));
    }

    #[tokio::test]
    async fn console_login_denied_when_dev_mode_off() {
        let state = test_state(false);
        let req = Request::builder()
            .method("POST")
            .uri("/console/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(header::COOKIE, format!("{CSRF_COOKIE}=t1"))
            .body(Body::from("csrf_token=t1&oidc_code=alice:user"))
            .unwrap();
        let resp = console_app(&state).oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let body = String::from_utf8_lossy(&body).to_string();
        assert!(body.contains("占位登录已禁用"), "dev_mode=false 控制台占位登录必须拒绝");
        assert!(state
            .audit
            .snapshot()
            .iter()
            .any(|e| e.event == "auth_placeholder_denied"), "审计事件必须记录");
    }

    #[tokio::test]
    async fn console_login_requires_csrf() {
        let state = test_state(true);
        let req = Request::builder()
            .method("POST")
            .uri("/console/login")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("oidc_code=alice:user"))
            .unwrap();
        let resp = console_app(&state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), Code::FORBIDDEN, "缺失 CSRF 拒绝");
    }

    #[tokio::test]
    async fn dashboard_without_session_redirects_to_login() {
        let state = test_state(false);
        let resp = console_app(&state)
            .oneshot(
                Request::builder()
                    .uri("/console/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), Code::SEE_OTHER);
        assert_eq!(resp.headers().get("location").and_then(|v| v.to_str().ok()), Some("/"));
    }

    #[tokio::test]
    async fn admin_sees_policy_and_audit_user_does_not() {
        let state = test_state(false);
        let dashboard_for = |state: &Arc<AppState>, roles: &[&str]| {
            let token = state.auth.issue_token("viewer", roles).unwrap();
            let app = console_app(state);
            async move {
                app.oneshot(
                    Request::builder()
                        .uri("/console/dashboard")
                        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };
        let resp = dashboard_for(&state, &["admin"]).await;
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let admin_body = String::from_utf8_lossy(&body).to_string();
        assert!(admin_body.contains("策略矩阵"));
        assert!(admin_body.contains("审计检索"));
        assert!(admin_body.contains("ssh-jump01"));

        let resp = dashboard_for(&state, &["user"]).await;
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let user_body = String::from_utf8_lossy(&body).to_string();
        assert!(!user_body.contains("策略矩阵"));
        assert!(!user_body.contains("审计检索"));
        assert!(user_body.contains("ssh-jump01"));
    }

    #[test]
    fn session_cookie_flags_and_csrf() {
        let cookie = session_cookie("tok123");
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains(&format!("Max-Age={CONSOLE_SESSION_TTL}")));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=x; ezs_csrf=tok; more=y"),
        );
        assert_eq!(cookie_value(&headers, CSRF_COOKIE).as_deref(), Some("tok"));
        assert_eq!(cookie_value(&headers, SESSION_COOKIE), None);
    }
}
