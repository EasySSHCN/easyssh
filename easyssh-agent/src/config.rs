//! agent.toml 解析（方案第 5 节：gateway url、resource_id、local_backend、
//! token 落盘形态；重连/心跳参数按 SSH 长会话保守口径给出默认值）。
//!
//! `AgentConfig::parse` 从 TOML 字符串解析（便于单测，不写临时文件）；
//! `load_from_path` 读文件后复用同一入口。

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// 默认本地后端：目标机 sshd。
pub const DEFAULT_LOCAL_BACKEND: &str = "127.0.0.1:22";
/// 默认心跳间隔秒（方案：30s）。
pub const DEFAULT_HEARTBEAT_SECS: u64 = 30;
/// 默认 DIAL 后等待 OPEN_OK 超时秒（与网关 dial_timeout_secs 口径一致）。
pub const DEFAULT_DIAL_TIMEOUT_SECS: u64 = 10;
/// 默认单连接并发 session 上限（与网关 max_sessions_per_conn 一致）。
pub const DEFAULT_MAX_SESSIONS: usize = 8;
/// 退避基值毫秒。
pub const DEFAULT_BACKOFF_BASE_MS: u64 = 1_000;
/// 退避封顶毫秒（方案：30s）。
pub const DEFAULT_BACKOFF_CAP_MS: u64 = 30_000;

/// 默认配置文件路径：`agent.toml` 与可执行文件同目录，其次当前目录。
pub fn default_config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("agent.toml");
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from("agent.toml")
}

/// 默认 token 落盘路径：
/// - Linux/Unix：`~/.config/easyssh-agent/agent-token`（0600）
/// - Windows：`%APPDATA%\EasySSHAgent\agent-token.bin`（DPAPI 加密 blob）
pub fn default_token_path() -> PathBuf {
    #[cfg(unix)]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config").join("easyssh-agent").join("agent-token")
    }
    #[cfg(windows)]
    {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(base).join("EasySSHAgent").join("agent-token.bin")
    }
    #[cfg(not(any(unix, windows)))]
    {
        PathBuf::from("agent-token")
    }
}

/// Agent 运行配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    /// 网关基址：`https://gw:8443` / `http://gw:8080` / `wss://gw:8443` 均可，
    /// 内部归一化（wss→https、ws→http，去掉尾部 `/`）。
    pub gateway_url: String,
    /// 绑定的资源 ID（AgentToken rid 与网关资源登记一致）。
    pub resource_id: String,
    /// 唯一允许 DIAL 的本地后端（默认 127.0.0.1:22）。
    pub local_backend: String,
    /// 心跳间隔秒。
    pub heartbeat_secs: u64,
    /// 本地 TCP connect 超时秒。
    pub dial_timeout_secs: u64,
    /// 单连接并发 session 上限。
    pub max_sessions: usize,
    /// 退避基值毫秒。
    pub backoff_base_ms: u64,
    /// 退避封顶毫秒。
    pub backoff_cap_ms: u64,
    /// 信任自签证书（首版放行开关；生产建议导入根证书后关闭）。
    pub accept_invalid_certs: bool,
    /// token 落盘路径（空 = 平台默认）。
    pub token_path: Option<PathBuf>,
}

/// TOML 文件形态（全部字段可选，缺省见 DEFAULT_*）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentConfigFile {
    pub gateway_url: Option<String>,
    pub resource_id: Option<String>,
    pub local_backend: Option<String>,
    pub heartbeat_secs: Option<u64>,
    pub dial_timeout_secs: Option<u64>,
    pub max_sessions: Option<usize>,
    pub backoff_base_ms: Option<u64>,
    pub backoff_cap_ms: Option<u64>,
    pub accept_invalid_certs: Option<bool>,
    pub token_path: Option<String>,
}

/// 配置解析错误。
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Parse(String),
    MissingField(&'static str),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Parse(e) => write!(f, "agent.toml 解析失败: {e}"),
            ConfigError::MissingField(k) => write!(f, "agent.toml 缺少必填字段: {k}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl AgentConfig {
    /// 归一化网关基址：wss→https、ws→http、去尾部 `/`。
    pub fn normalize_base(url: &str) -> String {
        let u = url.trim().trim_end_matches('/');
        if let Some(rest) = u.strip_prefix("wss://") {
            format!("https://{rest}")
        } else if let Some(rest) = u.strip_prefix("ws://") {
            format!("http://{rest}")
        } else {
            u.to_string()
        }
    }

    /// WS 端点：`{base}/agent/connect?resource_id={rid}`。
    pub fn ws_url(&self) -> String {
        let base = Self::normalize_base(&self.gateway_url);
        format!("{}/agent/connect?resource_id={}", base, self.resource_id)
    }

    /// enroll HTTP 端点基址（https/http）。
    pub fn http_base(&self) -> String {
        Self::normalize_base(&self.gateway_url)
    }

    /// 生效 token 落盘路径。
    pub fn token_path(&self) -> PathBuf {
        self.token_path.clone().unwrap_or_else(default_token_path)
    }

    /// 从 TOML 字符串解析。gateway_url/resource_id 必填。
    pub fn parse(toml_str: &str) -> Result<AgentConfig, ConfigError> {
        let file: AgentConfigFile =
            toml::from_str(toml_str).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let gateway_url = file
            .gateway_url
            .filter(|s| !s.trim().is_empty())
            .ok_or(ConfigError::MissingField("gateway_url"))?;
        let resource_id = file
            .resource_id
            .filter(|s| !s.trim().is_empty())
            .ok_or(ConfigError::MissingField("resource_id"))?;
        Ok(AgentConfig {
            gateway_url,
            resource_id,
            local_backend: file
                .local_backend
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_LOCAL_BACKEND.to_string()),
            heartbeat_secs: file.heartbeat_secs.unwrap_or(DEFAULT_HEARTBEAT_SECS).max(1),
            dial_timeout_secs: file
                .dial_timeout_secs
                .unwrap_or(DEFAULT_DIAL_TIMEOUT_SECS)
                .max(1),
            max_sessions: file.max_sessions.unwrap_or(DEFAULT_MAX_SESSIONS).max(1),
            backoff_base_ms: file.backoff_base_ms.unwrap_or(DEFAULT_BACKOFF_BASE_MS).max(1),
            backoff_cap_ms: file
                .backoff_cap_ms
                .unwrap_or(DEFAULT_BACKOFF_CAP_MS)
                .max(file.backoff_base_ms.unwrap_or(DEFAULT_BACKOFF_BASE_MS).max(1)),
            accept_invalid_certs: file.accept_invalid_certs.unwrap_or(false),
            token_path: file.token_path.map(PathBuf::from),
        })
    }

    /// 从文件路径加载（缺失/解析失败返回 Err）。
    pub fn load_from_path(path: &Path) -> Result<AgentConfig, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Parse(format!("读取 {}: {e}", path.display()))
        })?;
        Self::parse(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let c = AgentConfig::parse(
            r#"
gateway_url = "wss://gw.example.com:8443/"
resource_id = "u-abc123"
local_backend = "127.0.0.1:2222"
heartbeat_secs = 15
dial_timeout_secs = 5
max_sessions = 4
backoff_base_ms = 500
backoff_cap_ms = 10000
accept_invalid_certs = true
token_path = "/var/lib/easyssh-agent/token"
"#,
        )
        .expect("parse");
        assert_eq!(c.gateway_url, "wss://gw.example.com:8443/");
        assert_eq!(c.resource_id, "u-abc123");
        assert_eq!(c.local_backend, "127.0.0.1:2222");
        assert_eq!(c.heartbeat_secs, 15);
        assert_eq!(c.max_sessions, 4);
        assert!(c.accept_invalid_certs);
        assert_eq!(c.token_path().to_str(), Some("/var/lib/easyssh-agent/token"));
        // WS 端点归一化：wss → https 基址，去尾部斜杠
        assert_eq!(
            c.ws_url(),
            "https://gw.example.com:8443/agent/connect?resource_id=u-abc123"
        );
        assert_eq!(c.http_base(), "https://gw.example.com:8443");
    }

    #[test]
    fn parse_minimal_uses_spec_defaults() {
        let c = AgentConfig::parse(
            r#"
gateway_url = "http://127.0.0.1:8080"
resource_id = "agent-echo"
"#,
        )
        .expect("parse");
        assert_eq!(c.local_backend, DEFAULT_LOCAL_BACKEND);
        assert_eq!(c.heartbeat_secs, 30);
        assert_eq!(c.dial_timeout_secs, 10);
        assert_eq!(c.max_sessions, 8);
        assert_eq!(c.backoff_base_ms, 1_000);
        assert_eq!(c.backoff_cap_ms, 30_000);
        assert!(!c.accept_invalid_certs);
    }

    #[test]
    fn missing_required_fields_rejected() {
        assert_eq!(
            AgentConfig::parse(""),
            Err(ConfigError::MissingField("gateway_url"))
        );
        assert_eq!(
            AgentConfig::parse("gateway_url = \"wss://g\""),
            Err(ConfigError::MissingField("resource_id"))
        );
        assert!(matches!(
            AgentConfig::parse("gateway_url = 42"),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn normalize_base_variants() {
        assert_eq!(AgentConfig::normalize_base("wss://g:1/x/"), "https://g:1/x");
        assert_eq!(AgentConfig::normalize_base("ws://g"), "http://g");
        assert_eq!(AgentConfig::normalize_base("https://g/"), "https://g");
        assert_eq!(AgentConfig::normalize_base(" http://g "), "http://g");
    }

    #[test]
    fn cap_clamped_to_base() {
        let c = AgentConfig::parse(
            "gateway_url = \"http://g\"\nresource_id=\"r\"\nbackoff_base_ms = 8000\nbackoff_cap_ms = 1000",
        )
        .expect("parse");
        assert_eq!(c.backoff_cap_ms, 8000, "cap 至少不小于 base");
    }
}
