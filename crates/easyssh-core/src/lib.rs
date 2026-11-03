//! EasySSH v4 核心库：与 EasySSH 3.x (Electron) 二进制兼容的配置/加密层。
//!
//! 兼容协议（见 3.x main.js）：
//! - 会话密码字段落盘为 `enc:v1:<iv>:<tag>:<密文>`（AES-256-GCM，DEK=config.key 32 字节）
//! - 主密码启用后 DEK 被包裹为 `config.key.enc` = `wrap:v1:<salt>:<iv>:<tag>:<密文>`
//!   （KEK = PBKDF2-HMAC-SHA256(口令, salt, 100000, 32)）
//! - config.json 结构 { sessions: [...], settings: {...} }，sessions 在 3.x 内为
//!   整体加密或明文（见 config.rs 的读取兼容）

pub mod config;
pub mod crypto;
pub mod ota_sign;
