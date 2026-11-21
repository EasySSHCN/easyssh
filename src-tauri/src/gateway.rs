/* EasySSH v5.3 / R-20261120-005(②部分)：客户端网关接入（easyrdp-gateway）。
 *
 * 协议：
 * 1) POST https(s)://<gw>/api/auth/login
 *    请求体（按 auth 类型）：token → {token, resourceId[, otp]}；
 *    userpass → {username, password, resourceId[, otp]}；inherit → {resourceId[, otp]}。
 *    成功响应：{token, tokenTtlSeconds: 600}（10 分钟短期 JWT）。
 *    失败响应：{code: "otp_required"|"auth_placeholder_denied"|"resource_not_found"|...}。
 * 2) WSS /tunnel/<resource_id>?host=<目标host>&port=<目标port>
 *    （TLS，allow_self_signed=true 时跳过证书校验；目标信息经查询参数下发）
 * 3) WS 与 TCP 目标双向泵：产出 GenericStream（与 net.rs 现有流类型对接）。
 *
 * 密码/口令口径：gateway.password/token/otp 存于会话记录 gateway 字段内，
 * 落盘前用与顶层 password 相同的 DEK(AES-256-GCM, enc:v1:) 加密（DEK 文件走
 * Windows DPAPI，绑定本机用户），加载后解密；日志/错误/健康检查响应一律不回显。
 *
 * 失败分类（错误消息前缀与渲染端 FINGERPRINT_MISMATCH 同风格）：
 *   GATEWAY_UNREACHABLE / GATEWAY_AUTH_FAILED / GATEWAY_RESOURCE_NOT_FOUND /
 *   GATEWAY_OTP_REQUIRED（前端弹 6 位码后带 otp 重连）/
 *   GATEWAY_AUTH_PLACEHOLDER_DENIED（占位登录被拒 401）。
 * */
use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::net::GenericStream;
use easyssh_core::crypto::ENC_PREFIX;

/// 网关错误分类（错误码前缀，渲染端按前缀分支：OTP 弹码输入等）
pub const E_UNREACHABLE: &str = "GATEWAY_UNREACHABLE";
pub const E_AUTH_FAILED: &str = "GATEWAY_AUTH_FAILED";
pub const E_RESOURCE_NOT_FOUND: &str = "GATEWAY_RESOURCE_NOT_FOUND";
pub const E_OTP_REQUIRED: &str = "GATEWAY_OTP_REQUIRED";
pub const E_PLACEHOLDER_DENIED: &str = "GATEWAY_AUTH_PLACEHOLDER_DENIED";
pub const E_UNKNOWN: &str = "GATEWAY_UNKNOWN";

/// 登录短期 JWT 有效期口径（服务端约定 600s；仅用于注释/探测展示，不本地续期）
pub const TOKEN_TTL_SECONDS: u64 = 600;

/* ---------- 配置 ---------- */

/// 网关配置（存于会话记录一级字段 `gateway`；旧数据无此字段 = 不走网关，零迁移）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GatewayCfg {
    pub hostname: String,
    pub port: u16,
    pub use_tls: bool,
    pub allow_self_signed: bool,
    /// token | userpass | inherit
    pub auth: String,
    pub username: String,
    pub password: String,
    pub token: String,
    pub resource_id: String,
    /// 6 位动态码（OTP 重连时由前端带入；不持久化到导出/备份）
    pub otp: String,
    /// 备用网关（主网关失败时按序转移）
    pub fallbacks: Vec<GatewayCfg>,
}

impl Default for GatewayCfg {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            port: 443,
            use_tls: true,
            allow_self_signed: false,
            auth: "userpass".into(),
            username: String::new(),
            password: String::new(),
            token: String::new(),
            resource_id: String::new(),
            otp: String::new(),
            fallbacks: Vec::new(),
        }
    }
}

impl GatewayCfg {
    pub fn enabled(&self) -> bool {
        !self.hostname.is_empty() && !self.resource_id.is_empty()
    }
    fn gw_base(&self) -> String {
        let scheme = if self.use_tls { "https" } else { "http" };
        format!("{}://{}:{}", scheme, self.hostname, self.port)
    }
    pub fn login_url(&self) -> String {
        format!("{}/api/auth/login", self.gw_base())
    }
    /// WSS/WS 隧道地址（目标经查询参数下发）
    pub fn tunnel_url(&self, host: &str, port: u16) -> String {
        let scheme = if self.use_tls { "wss" } else { "ws" };
        format!(
            "{}://{}:{}/tunnel/{}?host={}&port={}",
            scheme,
            self.hostname,
            self.port,
            self.resource_id,
            urlenc(host),
            port
        )
    }
}

/// 最小 URL 组件转义（host 只可能含字母数字/./-/:，仅处理保留字符兜底）
fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' | '_' | '~' => out.push(c),
            _ => {
                let mut b = [0u8; 4];
                for byte in c.encode_utf8(&mut b).as_bytes() {
                    out.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    out
}

/* ---------- 失败分类 ---------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GwErrKind {
    Unreachable,
    AuthFailed,
    ResourceNotFound,
    OtpRequired,
    PlaceholderDenied,
    Unknown,
}

impl GwErrKind {
    pub fn code(self) -> &'static str {
        match self {
            GwErrKind::Unreachable => E_UNREACHABLE,
            GwErrKind::AuthFailed => E_AUTH_FAILED,
            GwErrKind::ResourceNotFound => E_RESOURCE_NOT_FOUND,
            GwErrKind::OtpRequired => E_OTP_REQUIRED,
            GwErrKind::PlaceholderDenied => E_PLACEHOLDER_DENIED,
            GwErrKind::Unknown => E_UNKNOWN,
        }
    }
    fn hint(self) -> &'static str {
        match self {
            GwErrKind::Unreachable => "建议: 检查网关地址/端口/网络连通性,或切换备用网关",
            GwErrKind::AuthFailed => "建议: 检查会话保存的网关账号/密码或令牌",
            GwErrKind::ResourceNotFound => "建议: 确认 resourceId 与网关侧资源映射一致",
            GwErrKind::OtpRequired => "建议: 输入 6 位动态验证码后重试",
            GwErrKind::PlaceholderDenied => "建议: 该账号为占位登录,请先在服务端完成真实绑定/激活",
            GwErrKind::Unknown => "建议: 查看网关服务端日志定位原因",
        }
    }
}

/// 拼装带错误码前缀的错误消息（渲染端按前缀分类弹窗，正文风格与跳板链诊断一致）
pub fn gw_err_msg(kind: GwErrKind, gw: &GatewayCfg, detail: &str) -> String {
    let desc = match kind {
        GwErrKind::Unreachable => "网关连接失败",
        GwErrKind::AuthFailed => "网关认证失败",
        GwErrKind::ResourceNotFound => "网关资源不存在",
        GwErrKind::OtpRequired => "网关认证需要 OTP 验证码",
        GwErrKind::PlaceholderDenied => "网关拒绝占位登录",
        GwErrKind::Unknown => "网关错误",
    };
    format!(
        "{}: 网关 {}:{} {}{}",
        kind.code(),
        gw.hostname,
        gw.port,
        if detail.is_empty() { desc.to_string() } else { format!("{desc}: {detail}") },
        if let Some(s) = nonempty_hint(kind) { format!("\n{s}") } else { String::new() }
    )
}

fn nonempty_hint(kind: GwErrKind) -> Option<String> {
    Some(kind.hint().to_string())
}

/// 服务端错误响应 → 分类。body 为 JSON 时优先取 `code` 字段，
/// 否则按 HTTP 状态码归类（401 认证 / 404 资源 / 其他未知）。
pub fn classify_login_failure(status: Option<u16>, body: &str) -> GwErrKind {
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(|s| s.to_string()))
        .or_else(|| {
            serde_json::from_str::<Value>(body)
                .ok()
                .filter(|v| v.get("otpRequired").and_then(|x| x.as_bool()).unwrap_or(false))
                .map(|_| "otp_required".to_string())
        });
    match code.as_deref() {
        Some("otp_required") => return GwErrKind::OtpRequired,
        Some("auth_placeholder_denied") => return GwErrKind::PlaceholderDenied,
        Some("resource_not_found") => return GwErrKind::ResourceNotFound,
        _ => {}
    }
    match status {
        Some(401) | Some(403) => GwErrKind::AuthFailed,
        Some(404) => GwErrKind::ResourceNotFound,
        Some(_) => GwErrKind::Unknown,
        None => GwErrKind::Unreachable,
    }
}

/* ---------- TLS（复用 rustls；allow_self_signed 时危险校验器放行） ---------- */

#[derive(Debug)]
struct DangerousVerifier;

impl rustls::client::danger::ServerCertVerifier for DangerousVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        use rustls::SignatureScheme as S;
        vec![
            S::ED25519,
            S::ECDSA_NISTP256_SHA256,
            S::ECDSA_NISTP384_SHA384,
            S::RSA_PSS_SHA256,
            S::RSA_PSS_SHA384,
            S::RSA_PSS_SHA512,
            S::RSA_PKCS1_SHA256,
            S::RSA_PKCS1_SHA384,
            S::RSA_PKCS1_SHA512,
        ]
    }
}

/// 网关 TLS 连接器配置（自签名放行或系统根证书）
pub fn tls_config(allow_self_signed: bool) -> Result<Arc<rustls::ClientConfig>, String> {
    let builder = rustls::ClientConfig::builder();
    let cfg = if allow_self_signed {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(DangerousVerifier))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        match rustls_native_certs::load_native_certs() {
            Ok(certs) => {
                for c in certs {
                    let _ = roots.add(c);
                }
            }
            Err(e) => return Err(format!("加载系统根证书失败: {e}")),
        }
        if roots.is_empty() {
            // 系统根证书取不到时兜底 webpki 根（覆盖主流 CA）
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

type TlsStream = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;

/// 明文/TLS 统一底层流（WSS 升级与泵共用，两分支类型对齐）
enum GwStream {
    Plain(tokio::net::TcpStream),
    Tls(TlsStream),
}

use std::pin::Pin;
use std::task::{Context, Poll};

impl AsyncRead for GwStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            GwStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            GwStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for GwStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            GwStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            GwStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            GwStream::Plain(s) => Pin::new(s).poll_flush(cx),
            GwStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            GwStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            GwStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

async fn gw_tcp_tls(gw: &GatewayCfg) -> Result<TlsStream, String> {
    let tcp = tokio::net::TcpStream::connect((gw.hostname.as_str(), gw.port))
        .await
        .map_err(|e| format!("连接网关 {}:{} 失败: {e}", gw.hostname, gw.port))?;
    tcp.set_nodelay(true).ok();
    if !gw.use_tls {
        // 明文 WS 场景：登录仍按 http 明文走（无 TLS 包裹）；此处仅 TLS 路径被调用
        unreachable!("gw_tcp_tls 仅在 use_tls=true 时调用");
    }
    let connector = tokio_rustls::TlsConnector::from(tls_config(gw.allow_self_signed)?);
    let name = rustls::pki_types::ServerName::try_from(gw.hostname.clone())
        .map_err(|e| format!("网关主机名无效: {e}"))?;
    let tls = connector
        .connect(name, tcp)
        .await
        .map_err(|e| format!("网关 TLS 握手失败: {e}"))?;
    Ok(tls)
}

/* ---------- 登录（换短期 JWT） ---------- */

fn login_body(gw: &GatewayCfg) -> String {
    let mut body = serde_json::Map::new();
    body.insert("resourceId".into(), json!(gw.resource_id));
    match gw.auth.as_str() {
        "token" => {
            body.insert("token".into(), json!(gw.token));
        }
        "inherit" => {} // 继承型：仅 resourceId，凭据由网关侧会话继承
        _ => {
            // userpass（默认）
            body.insert("username".into(), json!(gw.username));
            body.insert("password".into(), json!(gw.password));
        }
    }
    if !gw.otp.is_empty() {
        body.insert("otp".into(), json!(gw.otp));
    }
    Value::Object(body).to_string()
}

pub struct LoginOutcome {
    pub token: String,
}

/// POST /api/auth/login → {token, tokenTtlSeconds:600}
pub async fn gateway_login(gw: &GatewayCfg) -> Result<LoginOutcome, String> {
    let body = login_body(gw);
    let req = format!(
        "POST /api/auth/login HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nUser-Agent: EasySSH/{}\r\n\r\n{}",
        gw.hostname,
        gw.port,
        body.len(),
        env!("CARGO_PKG_VERSION"),
        body
    );
    // 明文网关（use_tls=false）走裸 TCP；TLS 走 rustls（含自签名放行）
    let resp_bytes = if gw.use_tls {
        let mut tls = gw_tcp_tls(gw).await?;
        tls.write_all(req.as_bytes()).await.map_err(|e| format!("网关请求发送失败: {e}"))?;
        let mut out = Vec::with_capacity(4096);
        tls.read_to_end(&mut out).await.map_err(|e| format!("网关响应读取失败: {e}"))?;
        out
    } else {
        let mut tcp = tokio::net::TcpStream::connect((gw.hostname.as_str(), gw.port))
            .await
            .map_err(|e| format!("连接网关 {}:{} 失败: {e}", gw.hostname, gw.port))?;
        tcp.write_all(req.as_bytes()).await.map_err(|e| format!("网关请求发送失败: {e}"))?;
        let mut out = Vec::with_capacity(4096);
        tcp.read_to_end(&mut out).await.map_err(|e| format!("网关响应读取失败: {e}"))?;
        out
    };
    let resp = String::from_utf8_lossy(&resp_bytes);
    let (head, body_txt) = match resp.find("\r\n\r\n") {
        Some(i) => (&resp[..i], resp[i + 4..].to_string()),
        None => (resp.as_ref(), String::new()),
    };
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok());
    if status == Some(200) {
        let v: Value = serde_json::from_str(body_txt.trim())
            .map_err(|e| format!("网关登录响应解析失败: {e}"))?;
        let token = v
            .get("token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("{}: 网关登录响应缺少 token 字段", E_UNKNOWN))?;
        Ok(LoginOutcome { token: token.to_string() })
    } else {
        let kind = classify_login_failure(status, body_txt.trim());
        Err(gw_err_msg(kind, gw, body_txt.trim()))
    }
}

/* ---------- WSS 隧道 + 双向泵 ---------- */

/// 建立网关隧道流：登录换 JWT → WSS /tunnel/<resource_id> → WS⇄TCP 双向泵。
/// 返回的 GenericStream 对调用方透明（等价一条到目标 host:port 的 TCP 流）。
pub async fn gateway_stream(gw: &GatewayCfg, host: &str, port: u16) -> Result<GenericStream, String> {
    // 1) 登录（短期 JWT，600s 口径；仅作网关准入，不透传给目标）
    gateway_login(gw).await?;
    // 2) WSS/WS 升级（握手请求由 tokio-tungstenite 按 URL 自行写入；底层流统一封装）
    let ws_url = gw.tunnel_url(host, port);
    let underlying = if gw.use_tls {
        GwStream::Tls(gw_tcp_tls(gw).await?)
    } else {
        let tcp = tokio::net::TcpStream::connect((gw.hostname.as_str(), gw.port))
            .await
            .map_err(|e| gw_err_msg(GwErrKind::Unreachable, gw, &format!("连接失败: {e}")))?;
        tcp.set_nodelay(true).ok();
        GwStream::Plain(tcp)
    };
    let conn = if gw.use_tls {
        tokio_tungstenite::Connector::Rustls(tls_config(gw.allow_self_signed)?)
    } else {
        tokio_tungstenite::Connector::Plain
    };
    let ws = tokio_tungstenite::client_async_tls_with_config(ws_url, underlying, None, Some(conn))
        .await
        .map_err(|e| gw_err_msg(GwErrKind::Unreachable, gw, &format!("WSS 升级失败: {e}")))?
        .0;
    // 3) WS⇄TCP 双向泵：对端经 tokio duplex 对接（net::GenericStream）
    let (mut client_io, gw_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let (mut sink, mut src) = ws.split();
        let (mut gr, mut gwr) = tokio::io::split(gw_io);
        let up = async move {
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match gr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if sink.send(tokio_tungstenite::tungstenite::Message::Binary(buf[..n].to_vec())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = sink.close().await;
        };
        let down = async move {
            while let Some(msg) = src.next().await {
                match msg {
                    Ok(tokio_tungstenite::tungstenite::Message::Binary(b)) => {
                        if gwr.write_all(&b).await.is_err() {
                            break;
                        }
                    }
                    Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue, // Ping/Pong/Text 由底层处理或忽略
                }
            }
            let _ = gwr.flush().await;
        };
        tokio::join!(up, down);
        // 泵退出即 drop duplex 双端 → client_io 读写端收到 EOF/错误，上层 SSH 会话随之终止
    });
    Ok(Box::new(client_io))
}

/* ---------- 健康探测 ---------- */

/// 单网关探测：TCP(+TLS) 握手时延，成功=绿 / 失败=红（错误文案不带凭据）。
async fn probe_one(gw: &GatewayCfg) -> Value {
    let start = Instant::now();
    let mut out = json!({
        "hostname": gw.hostname,
        "port": gw.port,
        "useTls": gw.use_tls,
    });
    if gw.hostname.is_empty() {
        out["ok"] = json!(false);
        out["status"] = json!("red");
        out["error"] = json!("网关地址未配置");
        return out;
    }
    let res: Result<(), String> = async {
        let tcp = tokio::net::TcpStream::connect((gw.hostname.as_str(), gw.port))
            .await
            .map_err(|e| format!("TCP 连接失败: {e}"))?;
        if gw.use_tls {
            let connector = tokio_rustls::TlsConnector::from(tls_config(gw.allow_self_signed)?);
            let name = rustls::pki_types::ServerName::try_from(gw.hostname.clone())
                .map_err(|e| format!("主机名无效: {e}"))?;
            connector.connect(name, tcp).await.map_err(|e| format!("TLS 握手失败: {e}"))?;
        }
        Ok(())
    }
    .await;
    let ms = start.elapsed().as_millis() as u64;
    match res {
        Ok(()) => {
            out["ok"] = json!(true);
            out["status"] = json!("green");
            out["latencyMs"] = json!(ms);
        }
        Err(e) => {
            out["ok"] = json!(false);
            out["status"] = json!("red");
            out["latencyMs"] = json!(ms);
            out["error"] = json!(e);
        }
    }
    out
}

/// 主 + 备用网关并发探测（flatten 到一层结果数组，保持主在前、fallbacks 按序在后）
pub async fn probe_gateway_health(gw: &GatewayCfg) -> Vec<Value> {
    let mut jobs = Vec::new();
    if gw.enabled() {
        let gw = gw.clone();
        jobs.push(tokio::spawn(async move { probe_one(&gw).await }));
    }
    for fb in &gw.fallbacks {
        if fb.enabled() {
            let fb = fb.clone();
            jobs.push(tokio::spawn(async move { probe_one(&fb).await }));
        }
    }
    let mut futs = Vec::new();
    for j in jobs {
        if let Ok(v) = j.await {
            futs.push(v);
        }
    }
    futs
}

/* ---------- 会话记录 gateway 秘密字段（与顶层 password 同口径加密/脱敏） ---------- */

pub const GW_SECRET_FIELDS: [&str; 3] = ["password", "token", "otp"];

fn for_each_gw_secret_mut(v: &mut Value, f: &mut impl FnMut(&mut String)) {
    // 会话记录.gateway（含 fallbacks 递归）
    let gws: Vec<*mut Value> = match v.get_mut("gateway") {
        Some(g) => {
            let mut ptrs = vec![g as *mut Value];
            if let Some(fbs) = g.get_mut("fallbacks").and_then(|x| x.as_array_mut()) {
                for fb in fbs {
                    ptrs.push(fb as *mut Value);
                }
            }
            ptrs
        }
        None => return,
    };
    for ptr in gws {
        // SAFETY: 指针来自同一 &mut Value 的不同字段路径，互不别名
        let g: &mut Value = unsafe { &mut *ptr };
        for field in GW_SECRET_FIELDS {
            if let Some(Value::String(s)) = g.get_mut(field) {
                f(s);
            }
        }
    }
}

/// 落盘前加密 gateway 秘密字段（与顶层 password 相同 enc:v1: 格式 + DEK，
/// DEK 文件由 easyssh-core DPAPI 保护——即现有密码存储口径）
pub fn encrypt_gateway_secrets(sessions: &mut Value, f: impl Fn(&str) -> Result<String, String>) {
    if let Some(arr) = sessions.as_array_mut() {
        for s in arr {
            for_each_gw_secret_mut(s, &mut |s: &mut String| {
                if !s.is_empty() && !s.starts_with(ENC_PREFIX) {
                    if let Ok(enc) = f(s) {
                        *s = enc;
                    }
                }
            });
        }
    }
}

/// 加载后解密 gateway 秘密字段
pub fn decrypt_gateway_secrets(sessions: &mut Value, f: impl Fn(&str) -> Result<String, String>) {
    if let Some(arr) = sessions.as_array_mut() {
        for s in arr {
            for_each_gw_secret_mut(s, &mut |s: &mut String| {
                if s.starts_with(ENC_PREFIX) {
                    if let Ok(plain) = f(s) {
                        *s = plain;
                    }
                }
            });
        }
    }
}

/// 备份/导出脱敏：剥离 gateway password/token/otp（主 + fallbacks）
pub fn strip_gateway_secrets(value: &mut Value) {
    if let Some(arr) = value.as_array_mut() {
        for s in arr {
            for_each_gw_secret_mut(s, &mut |s: &mut String| s.clear());
        }
    }
}

/// 从会话记录解析 gateway 配置（无字段/字段残缺 → None = 不走网关）
pub fn gateway_from_record(s: &Value) -> Option<GatewayCfg> {
    serde_json::from_value::<GatewayCfg>(s.get("gateway")?.clone())
        .ok()
        .filter(|g| g.enabled())
}

/* ---------- 单元测试 ---------- */
#[cfg(test)]
mod tests {
    use super::*;

    fn gw() -> GatewayCfg {
        GatewayCfg {
            hostname: "gw.example.com".into(),
            port: 7443,
            use_tls: true,
            allow_self_signed: true,
            auth: "userpass".into(),
            username: "ops".into(),
            password: "pw".into(),
            token: String::new(),
            resource_id: "res-1".into(),
            otp: String::new(),
            fallbacks: Vec::new(),
        }
    }

    #[test]
    fn urls_and_login_body() {
        let g = gw();
        assert_eq!(g.login_url(), "https://gw.example.com:7443/api/auth/login");
        assert_eq!(
            g.tunnel_url("10.0.0.5", 22),
            "wss://gw.example.com:7443/tunnel/res-1?host=10.0.0.5&port=22"
        );
        let body: Value = serde_json::from_str(&login_body(&g)).unwrap();
        assert_eq!(body["username"], "ops");
        assert_eq!(body["resourceId"], "res-1");
        let tok = GatewayCfg { auth: "token".into(), token: "T".into(), ..gw() };
        let tb: Value = serde_json::from_str(&login_body(&tok)).unwrap();
        assert_eq!(tb["token"], "T");
        assert!(tb.get("username").is_none());
    }

    #[test]
    fn failure_classification() {
        assert_eq!(classify_login_failure(Some(200), r#"{"code":"otp_required"}"#), GwErrKind::OtpRequired);
        assert_eq!(classify_login_failure(Some(401), r#"{"code":"auth_placeholder_denied"}"#), GwErrKind::PlaceholderDenied);
        assert_eq!(classify_login_failure(Some(404), r#"{"code":"resource_not_found"}"#), GwErrKind::ResourceNotFound);
        assert_eq!(classify_login_failure(Some(401), r#"{"error":"bad"}"#), GwErrKind::AuthFailed);
        assert_eq!(classify_login_failure(Some(404), "not json"), GwErrKind::ResourceNotFound);
        assert_eq!(classify_login_failure(None, ""), GwErrKind::Unreachable);
        // otpRequired 布尔字段口径
        assert_eq!(classify_login_failure(Some(200), r#"{"otpRequired":true}"#), GwErrKind::OtpRequired);
    }

    #[test]
    fn err_msg_prefix_and_no_secret_echo() {
        let g = gw();
        let m = gw_err_msg(GwErrKind::OtpRequired, &g, "");
        assert!(m.starts_with(E_OTP_REQUIRED));
        assert!(!m.contains("pw"));
        let m2 = gw_err_msg(GwErrKind::AuthFailed, &g, "HTTP 401");
        assert!(m2.starts_with(E_AUTH_FAILED));
        assert!(m2.contains("建议"));
    }

    #[test]
    fn gateway_record_parse_and_default_off() {
        let rec = serde_json::json!({
            "host": "h", "port": 22,
            "gateway": {
                "hostname": "gw", "port": 7443, "useTls": true, "allowSelfSigned": false,
                "auth": "token", "token": "T", "resourceId": "r1",
                "fallbacks": [{ "hostname": "gw2", "resourceId": "r2" }]
            }
        });
        let g = gateway_from_record(&rec).unwrap();
        assert_eq!(g.port, 7443);
        assert_eq!(g.fallbacks.len(), 1);
        assert_eq!(g.fallbacks[0].port, 443); // 默认端口
        assert!(gateway_from_record(&serde_json::json!({"host": "h"})).is_none());
    }

    #[test]
    fn secret_walk_encrypt_decrypt_strip() {
        use easyssh_core::crypto::{decrypt_secret, encrypt_secret};
        let dek = [7u8; 32];
        let mut sessions = serde_json::json!([
            { "host": "h", "gateway": { "hostname": "gw", "resourceId": "r1",
                "password": "pw1", "token": "T1", "otp": "123456",
                "fallbacks": [{ "hostname": "gw2", "resourceId": "r2", "password": "pw2" }] } },
            { "host": "plain" }
        ]);
        encrypt_gateway_secrets(&mut sessions, |v| encrypt_secret(v, &dek));
        let g = &sessions[0]["gateway"];
        assert!(g["password"].as_str().unwrap().starts_with(ENC_PREFIX));
        assert!(g["fallbacks"][0]["password"].as_str().unwrap().starts_with(ENC_PREFIX));
        assert_eq!(g["hostname"], "gw"); // 非秘密字段不动
        decrypt_gateway_secrets(&mut sessions, |v| decrypt_secret(v, &dek));
        assert_eq!(sessions[0]["gateway"]["password"], "pw1");
        assert_eq!(sessions[0]["gateway"]["fallbacks"][0]["password"], "pw2");
        strip_gateway_secrets(&mut sessions);
        assert_eq!(sessions[0]["gateway"]["password"], "");
        assert_eq!(sessions[0]["gateway"]["token"], "");
        assert_eq!(sessions[0]["gateway"]["otp"], "");
        assert_eq!(sessions[0]["gateway"]["fallbacks"][0]["password"], "");
    }

    #[test]
    fn tls_config_builds_both_modes() {
        assert!(tls_config(true).is_ok());
        // 系统根证书加载失败也不该 panic（返回 Err 即可，CI 无证书库环境）
        let _ = tls_config(false);
    }
}
