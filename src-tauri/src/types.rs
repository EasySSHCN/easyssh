//! 连接选项等共享类型（渲染端 → 后端命令参数）。
use serde::Deserialize;

/// v5(M2)/F-M2-05：会话级前置代理配置（存于会话记录的 `proxy` 字段；
/// 旧数据无此字段时按 None 处理 = 无代理，v4 sessions 完全兼容）。
#[derive(Debug, Clone, Default, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxyCfg {
    /// none | socks5 | http（空串按 none）
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyCfg {
    pub fn enabled(&self) -> bool {
        matches!(self.kind.as_str(), "socks5" | "http") && !self.host.is_empty()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectOpts {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(alias = "username")]
    pub user: String,
    #[serde(default)]
    pub authtype: String, // password | key（兼容字段：3.x 重连路径不传，按 privateKey 是否提供判定）
    #[serde(default)]
    pub username: Option<String>, // 3.x 字段名（已由 user 的 alias 覆盖，占位避免误报）
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub private_key: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub expected_fingerprint: Option<String>,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
}

fn default_port() -> u16 {
    22
}
fn default_cols() -> u32 {
    80
}
fn default_rows() -> u32 {
    24
}
