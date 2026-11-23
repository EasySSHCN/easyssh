//! 配置模块：config.toml 解析（server / auth / audit / admin / tls / users /
//! oidc / policy / resources）。
//!
//! `GatewayConfig::parse` 从 TOML 字符串解析（便于单测，不写临时文件）；
//! `load_from_path` 读文件后复用同一解析入口，文件缺失时返回 None
//! （main 使用默认值 / 环境变量回退）。

use serde::Deserialize;
use std::path::Path;

use crate::policy::PolicyConfig;
use crate::resources::ResourceConfig;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GatewayConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub audit: AuditConfig,
    /// 管理接口配置（GET /admin/audit?limit=N，X-Admin-Key 鉴权）
    #[serde(default)]
    pub admin: AdminConfig,
    /// 策略矩阵：资源 × 角色(admin/user/readonly) × 动作(connect/list)；
    /// 缺省 = deny by default、admin 全通。
    #[serde(default)]
    pub policy: PolicyConfig,
    /// TLS 配置（cert_path/key_path 均配置时网关直跑 HTTPS/WSS；支持自签证书）
    #[serde(default)]
    pub tls: TlsConfig,
    /// 用户表（[users.<username>] totp_secret = Base32 密钥；配置了的用户
    /// 登录要求 OTP 二次认证）
    #[serde(default)]
    pub users: std::collections::BTreeMap<String, UserConfig>,
    /// [oidc] 段 —— 真实 IdP 对接（Authorization Code 流）。字段不全时
    /// `enabled() == false`。
    #[serde(default)]
    pub oidc: OidcConfig,
    /// R-20261122-004：[agent] 段 —— 目标机 Agent 反向隧道通道。
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub resources: Vec<ResourceConfig>,
}

/// R-20261122-004：[agent] 段配置。
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Agent 通道总开关（缺省 false；启用后挂载 GET /agent/connect）。
    #[serde(default)]
    pub enabled: bool,
    /// 单 Agent 连接并发 session 上限（默认 8）。
    #[serde(default)]
    pub max_sessions_per_conn: Option<usize>,
    /// 应用层心跳间隔秒（默认 30；连续 3 次未收到判死）。
    #[serde(default)]
    pub heartbeat_secs: Option<u64>,
    /// DIAL 后等待 OPEN_OK 的超时秒（默认 10）。
    #[serde(default)]
    pub dial_timeout_secs: Option<u64>,
    /// 每用户动态资源配额（默认 3；仅限登记台数）。
    #[serde(default)]
    pub max_resources_per_user: Option<usize>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_sessions_per_conn: None,
            heartbeat_secs: None,
            dial_timeout_secs: None,
            max_resources_per_user: None,
        }
    }
}

impl AgentConfig {
    pub fn max_sessions_per_conn(&self) -> usize {
        self.max_sessions_per_conn.unwrap_or(8).max(1)
    }

    pub fn heartbeat_secs(&self) -> u64 {
        self.heartbeat_secs.unwrap_or(30).max(1)
    }

    pub fn dial_timeout_secs(&self) -> u64 {
        self.dial_timeout_secs.unwrap_or(10).max(1)
    }

    pub fn max_resources_per_user(&self) -> usize {
        self.max_resources_per_user.unwrap_or(3).max(1)
    }

    /// 心跳判死静默窗口秒（heartbeat × 3；SSH 长会话保守口径）。
    pub fn silence_window_secs(&self) -> u64 {
        self.heartbeat_secs().saturating_mul(3)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerConfig {
    #[serde(default)]
    pub listen_addr: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuthConfig {
    #[serde(default)]
    pub jwt_secret: Option<String>,
    #[serde(default)]
    pub token_ttl_seconds: Option<u64>,
    /// R-20261120-006/008：OIDC 占位解析总闸。**默认 false（生产口径）**。
    /// 只有 dev_mode=true 且来源 IP 命中 dev_mode_allow_cidrs 时，占位
    /// `{"oidc_code":"用户:角色"}` 才放行；否则 401 + auth_placeholder_denied。
    #[serde(default)]
    pub dev_mode: bool,
    /// dev_mode 下允许调用占位登录的来源 CIDR 列表（逗号分隔见 toml 数组）。
    /// 空列表 = 仅允许回环 127.0.0.0/8 与 ::1。任一条非法启动即失败。
    #[serde(default)]
    pub dev_mode_allow_cidrs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuditConfig {
    #[serde(default)]
    pub ring_capacity: Option<usize>,
}

/// 管理接口配置。`admin_key` 为 GET /admin/audit 的共享密钥（X-Admin-Key）；
/// 生产环境请使用 >= 32 字节随机值（如 openssl rand -hex 32），绝不入库。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AdminConfig {
    #[serde(default)]
    pub admin_key: Option<String>,
}

/// [tls] 段。cert_path 与 key_path 同时配置 → axum-server rustls 直跑
/// HTTPS/WSS（自签证书即可，客户端需勾选信任自签）；未配置 → HTTP。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TlsConfig {
    #[serde(default)]
    pub cert_path: Option<String>,
    #[serde(default)]
    pub key_path: Option<String>,
}

impl TlsConfig {
    /// 是否启用网关本体 TLS（两个路径都非空才启用）。
    pub fn enabled(&self) -> bool {
        self.cert_path.as_deref().map_or(false, |s| !s.trim().is_empty())
            && self.key_path.as_deref().map_or(false, |s| !s.trim().is_empty())
    }
}

/// [users.<username>] 条目。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserConfig {
    /// Base32（RFC 4648）TOTP 密钥；配置后该用户登录要求 OTP 二次认证。
    #[serde(default)]
    pub totp_secret: Option<String>,
}

/// [oidc] 段 —— OIDC Authorization Code 流对接真实 IdP（code 换票主路径）。
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OidcConfig {
    /// IdP issuer（与 id_token `iss` claim 精确匹配）。
    #[serde(default)]
    pub issuer: Option<String>,
    /// OIDC discovery 文档 URL（缺省 = `{issuer}/.well-known/openid-configuration`）。
    #[serde(default)]
    pub discovery_url: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
    /// 回调地址，须先在 IdP 侧登记（如 https://gw.example.com/oidc/callback）。
    #[serde(default)]
    pub redirect_uri: Option<String>,
    /// scope 空格分隔（缺省 "openid profile email"）。
    #[serde(default)]
    pub scopes: Option<String>,
    /// id_token 中角色来源 claim 名（缺省 "roles"；为空时回退 "groups"，
    /// 再回退 ["user"]）。
    #[serde(default)]
    pub roles_claim: Option<String>,
}

impl OidcConfig {
    /// 四个必填字段（issuer/client_id/client_secret/redirect_uri）都非空才启用。
    pub fn enabled(&self) -> bool {
        [self.issuer.as_deref(), self.client_id.as_deref(), self.client_secret.as_deref(), self.redirect_uri.as_deref()]
            .iter()
            .all(|f| f.map_or(false, |s| !s.trim().is_empty()))
    }

    pub fn effective_scopes(&self) -> &str {
        self.scopes.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("openid profile email")
    }

    pub fn effective_roles_claim(&self) -> &str {
        self.roles_claim.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or("roles")
    }
}

impl GatewayConfig {
    pub fn parse(toml_text: &str) -> Result<Self, String> {
        toml::from_str(toml_text).map_err(|e| format!("config.toml 解析失败: {e}"))
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        Self::parse(&text).ok()
    }
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[server]
listen_addr = "127.0.0.1:8443"

[auth]
jwt_secret = "s3cret"
token_ttl_seconds = 600
dev_mode = false
dev_mode_allow_cidrs = ["10.0.0.0/8", "127.0.0.1"]

[audit]
ring_capacity = 1024

[admin]
admin_key = "REPLACE_WITH_RANDOM_ADMIN_KEY"

[tls]
cert_path = "/etc/easyssh/tls/server.crt"
key_path = "/etc/easyssh/tls/server.key"

[users.alice]
totp_secret = "JBSWY3DPEHPK3PXP"

[[resources]]
id = "ssh-jump01"
name = "Linux 跳板"
host = "10.0.0.5"
port = 22

[[policy.rules]]
resource = "*"
role = "user"
action = "connect"
decision = "allow"
"#;

    #[test]
    fn parse_full_config() {
        let cfg = GatewayConfig::parse(SAMPLE).expect("parse");
        assert_eq!(cfg.server.listen_addr.as_deref(), Some("127.0.0.1:8443"));
        assert_eq!(cfg.auth.jwt_secret.as_deref(), Some("s3cret"));
        assert_eq!(cfg.auth.token_ttl_seconds, Some(600));
        // R-20261120-006：dev_mode 总闸与 CIDR 白名单解析
        assert!(!cfg.auth.dev_mode, "生产口径 dev_mode=false");
        assert_eq!(cfg.auth.dev_mode_allow_cidrs, vec!["10.0.0.0/8".to_string(), "127.0.0.1".to_string()]);
        assert_eq!(cfg.audit.ring_capacity, Some(1024));
        assert_eq!(cfg.admin.admin_key.as_deref(), Some("REPLACE_WITH_RANDOM_ADMIN_KEY"));
        assert!(cfg.tls.enabled());
        assert_eq!(cfg.users["alice"].totp_secret.as_deref(), Some("JBSWY3DPEHPK3PXP"));
        assert_eq!(cfg.resources.len(), 1);
        assert_eq!(cfg.resources[0].id, "ssh-jump01");
        assert_eq!(cfg.resources[0].host, "10.0.0.5");
        assert_eq!(cfg.resources[0].port, 22);
        assert_eq!(cfg.policy.rules.len(), 1);
    }

    /// 空配置：dev_mode 必须缺省 false（R-20261120-006 安全默认值）。
    #[test]
    fn parse_empty_config_defaults() {
        let cfg = GatewayConfig::parse("").expect("parse empty");
        assert!(!cfg.auth.dev_mode);
        assert!(cfg.auth.dev_mode_allow_cidrs.is_empty());
        assert!(cfg.resources.is_empty());
        assert!(!cfg.tls.enabled());
        assert!(cfg.users.is_empty());
        assert!(cfg.policy.rules.is_empty());
        assert!(!cfg.oidc.enabled());
    }

    #[test]
    fn parse_oidc_partial_not_enabled() {
        let cfg = GatewayConfig::parse(
            "[oidc]\nissuer = \"https://sso.example.com\"\nclient_id = \"gw\"\n",
        )
        .expect("parse partial oidc");
        assert!(!cfg.oidc.enabled());
    }

    #[test]
    fn parse_invalid_toml_is_error() {
        assert!(GatewayConfig::parse("not [valid toml").is_err());
    }

    /// R-20261122-004：[agent] 段解析与缺省值。
    #[test]
    fn parse_agent_section_and_defaults() {
        // 缺省：通道关闭 + 各项默认值
        let empty = GatewayConfig::parse("").expect("empty");
        assert!(!empty.agent.enabled);
        assert_eq!(empty.agent.max_sessions_per_conn(), 8);
        assert_eq!(empty.agent.heartbeat_secs(), 30);
        assert_eq!(empty.agent.dial_timeout_secs(), 10);
        assert_eq!(empty.agent.max_resources_per_user(), 3);
        assert_eq!(empty.agent.silence_window_secs(), 90);
        // 显式配置
        let cfg = GatewayConfig::parse(
            "[agent]\nenabled = true\nmax_sessions_per_conn = 16\nheartbeat_secs = 20\ndial_timeout_secs = 5\nmax_resources_per_user = 5\n",
        )
        .expect("agent section");
        assert!(cfg.agent.enabled);
        assert_eq!(cfg.agent.max_sessions_per_conn(), 16);
        assert_eq!(cfg.agent.heartbeat_secs(), 20);
        assert_eq!(cfg.agent.dial_timeout_secs(), 5);
        assert_eq!(cfg.agent.max_resources_per_user(), 5);
        assert_eq!(cfg.agent.silence_window_secs(), 60);
    }
}
