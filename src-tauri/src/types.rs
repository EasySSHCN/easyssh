//! 连接选项等共享类型（渲染端 → 后端命令参数）。
use serde::Deserialize;

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
