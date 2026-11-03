/* EasySSH v5(M2)/F-M2-05：SOCKS5 / HTTP CONNECT 前置代理 + ProxyJump 跳板链。
 * - 前置代理：会话记录 `proxy` 字段（none/socks5/http + 地址/端口/可选用户名密码），
 *   连接时先与代理握手，再经代理建立到 SSH 服务器的 TCP。
 * - ProxyJump：会话记录 `jumpChain` 字段 = 其他已存会话的 key 列表
 *   ["host:port:user", ...]（最多 3 级，后端强制截断）。连接链：
 *   逐级登录跳板机（复用其已存凭据与其自身代理/跳板），并在其上开
 *   direct-tcpip 通道到下一级，最终建立到目标机的 TCP 流。
 * - 代理/跳板均从 config.json 的会话数据解析（与 v4 sessions 兼容：
 *   旧数据无 proxy/jumpChain 字段 = 直连）。 */
use std::sync::Arc;

use russh::client::Handle;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::configstate::ConfigState;
use crate::ssh::{authenticate, ClientHandler};
use crate::types::ProxyCfg;

/// 统一的隧道流：直连 TcpStream / 代理流 / direct-tcpip 通道流的装箱
pub trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> AsyncReadWrite for T {}
pub type GenericStream = Box<dyn AsyncReadWrite + Unpin + Send>;

/// 跳板链上的一级跳板（由已存会话解析而来）
#[derive(Debug, Clone)]
pub struct HopRec {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
    pub proxy: Option<ProxyCfg>,
}

pub const MAX_HOPS: usize = 3;

/* ---------- 会话记录解析（纯函数，便于单测） ---------- */

fn rec_str(s: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = s.get(*k).and_then(|x| x.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 会话记录 → 跳板信息（一级字段；jumpChain 的递归展开由 resolve_network 查表完成）
fn hop_from_record(s: &serde_json::Value) -> HopRec {
    let host = rec_str(s, &["host"]).unwrap_or_default();
    let port = s.get("port").and_then(|x| x.as_u64()).unwrap_or(22) as u16;
    let user = rec_str(s, &["username", "user"]).unwrap_or_default();
    let proxy = s
        .get("proxy")
        .and_then(|v| serde_json::from_value::<ProxyCfg>(v.clone()).ok())
        .filter(|p| p.enabled());
    HopRec {
        host,
        port,
        user,
        password: rec_str(s, &["password"]).unwrap_or_default(),
        private_key: rec_str(s, &["privateKey"]),
        passphrase: s
            .get("passphrase")
            .and_then(|x| x.as_str())
            .map(|v| v.to_string()),
        proxy,
    }
}

/// 两个会话记录是否同一目标（host:port:user 匹配）
fn record_matches(s: &serde_json::Value, host: &str, port: u16, user: &str) -> bool {
    s.get("host").and_then(|x| x.as_str()) == Some(host)
        && s.get("port").and_then(|x| x.as_u64()).unwrap_or(22) as u16 == port
        && (rec_str(s, &["username", "user"]).as_deref() == Some(user))
}

/// 从 config 会话数组解析跳板链：目标会话记录里的 jumpChain 逐 key 查表展开。
/// 返回 (前置代理, 跳板链)；链深度后端强制 ≤ 3。
pub fn resolve_network(
    config: &ConfigState,
    host: &str,
    port: u16,
    user: &str,
) -> (Option<ProxyCfg>, Vec<HopRec>) {
    let sessions = config
        .load_value()
        .ok()
        .and_then(|v| v.get("sessions").and_then(|x| x.as_array()).cloned())
        .unwrap_or_default();
    let self_rec = sessions.iter().find(|s| record_matches(s, host, port, user));
    let proxy = self_rec
        .and_then(|s| s.get("proxy"))
        .and_then(|v| serde_json::from_value::<ProxyCfg>(v.clone()).ok())
        .filter(|p| p.enabled());
    let mut hops = Vec::new();
    if let Some(chain) = self_rec.and_then(|s| s.get("jumpChain")).and_then(|x| x.as_array()) {
        for key in chain.iter().filter_map(|k| k.as_str()).take(MAX_HOPS) {
            let parts: Vec<&str> = key.splitn(3, ':').collect();
            if parts.len() != 3 {
                continue;
            }
            let (jh, jp) = (parts[0], parts[1].parse::<u16>().unwrap_or(22));
            let ju = parts[2];
            if let Some(rec) = sessions.iter().find(|s| record_matches(s, jh, jp, ju)) {
                // 防自环：跳板不得指向目标自身
                if record_matches(rec, host, port, user) {
                    continue;
                }
                hops.push(hop_from_record(rec));
            }
        }
    }
    hops.truncate(MAX_HOPS);
    (proxy, hops)
}

/* ---------- SOCKS5 / HTTP CONNECT（握手编解码为纯函数，便于单测） ---------- */

pub fn build_socks5_greeting(with_auth: bool) -> Vec<u8> {
    if with_auth {
        vec![0x05, 0x01, 0x02] // VER=5, 1 method, USERNAME/PASSWORD
    } else {
        vec![0x05, 0x01, 0x00] // NO AUTH
    }
}

pub fn build_socks5_auth(u: &str, p: &str) -> Vec<u8> {
    let mut out = vec![0x01u8, u.len() as u8];
    out.extend_from_slice(u.as_bytes());
    out.push(p.len() as u8);
    out.extend_from_slice(p.as_bytes());
    out
}

/// SOCKS5 CONNECT 请求（ATYP=domain）
pub fn build_socks5_connect(host: &str, port: u16) -> Vec<u8> {
    let mut out = vec![0x05u8, 0x01, 0x00, 0x03, host.len() as u8];
    out.extend_from_slice(host.as_bytes());
    out.push((port >> 8) as u8);
    out.push((port & 0xff) as u8);
    out
}

/// HTTP CONNECT 请求行/头
pub fn build_http_connect(host: &str, port: u16, auth_header: Option<&str>) -> String {
    let mut s = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if let Some(a) = auth_header {
        s.push_str(&format!("Proxy-Authorization: {a}\r\n"));
    }
    s.push_str("Proxy-Connection: keep-alive\r\n\r\n");
    s
}

/// 解析 HTTP CONNECT 响应：非 200 → 区分「代理拒绝」与「目标不可达」
pub fn parse_http_connect_response(buf: &[u8]) -> Result<(), String> {
    let text = String::from_utf8_lossy(buf);
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let _ver = parts.next();
    match parts.next().and_then(|c| c.parse::<u16>().ok()) {
        Some(200) => Ok(()),
        Some(code) => Err(format!("代理拒绝 CONNECT (HTTP {code}): {line}")),
        None => Err(format!("代理响应异常: {line}")),
    }
}

async fn read_http_headers(s: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        let n = s.read(&mut byte).await.map_err(|e| format!("读代理响应失败: {e}"))?;
        if n == 0 {
            return Err("代理连接被关闭".into());
        }
        buf.push(byte[0]);
        if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
            return Ok(buf);
        }
        if buf.len() > 16 * 1024 {
            return Err("代理响应头过大".into());
        }
    }
}

async fn socks5_handshake(
    s: &mut TcpStream,
    proxy: &ProxyCfg,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let with_auth = proxy.username.as_deref().map(|u| !u.is_empty()).unwrap_or(false);
    s.write_all(&build_socks5_greeting(with_auth))
        .await
        .map_err(|e| format!("代理握手失败: {e}"))?;
    let mut hdr = [0u8; 2];
    s.read_exact(&mut hdr).await.map_err(|e| format!("代理握手失败: {e}"))?;
    if hdr[0] != 0x05 {
        return Err("代理响应异常: 非 SOCKS5 协议".into());
    }
    if hdr[1] == 0xff {
        return Err("代理拒绝了认证方式(不接受用户名密码认证)".into());
    }
    if hdr[1] == 0x02 {
        let u = proxy.username.as_deref().unwrap_or("");
        let p = proxy.password.as_deref().unwrap_or("");
        s.write_all(&build_socks5_auth(u, p))
            .await
            .map_err(|e| format!("代理认证失败: {e}"))?;
        let mut ar = [0u8; 2];
        s.read_exact(&mut ar).await.map_err(|e| format!("代理认证失败: {e}"))?;
        if ar[1] != 0x00 {
            return Err("代理认证失败: 用户名或密码被拒绝".into());
        }
    } else if with_auth {
        return Err("代理拒绝了认证方式".into());
    }
    s.write_all(&build_socks5_connect(host, port))
        .await
        .map_err(|e| format!("代理请求失败: {e}"))?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.map_err(|e| format!("代理请求失败: {e}"))?;
    if head[1] != 0x00 {
        let reason = match head[1] {
            0x01 => "代理通用失败",
            0x02 => "代理规则不允许连接目标",
            0x03 => "目标网络不可达",
            0x04 => "目标主机不可达",
            0x05 => "代理连接目标被拒绝",
            0x06 => "TTL 过期",
            0x07 => "代理不支持该地址类型",
            _ => "代理连接失败",
        };
        return Err(format!("{reason} (SOCKS5 code {})", head[1]));
    }
    // 剩余变长地址 + 端口
    let skip = match head[3] {
        0x01 => 4 + 2,
        0x03 => {
            let mut l = [0u8; 1];
            s.read_exact(&mut l).await.map_err(|e| e.to_string())?;
            l[0] as usize + 2
        }
        0x04 => 16 + 2,
        _ => return Err("代理响应异常: 未知地址类型".into()),
    };
    let mut rest = vec![0u8; skip];
    s.read_exact(&mut rest).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn http_connect_handshake(
    s: &mut TcpStream,
    proxy: &ProxyCfg,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let auth_header = match proxy
        .username
        .as_deref()
        .filter(|u| !u.is_empty())
    {
        Some(u) => {
            use base64::Engine;
            let cred = format!(
                "{u}:{}",
                proxy.password.as_deref().unwrap_or("")
            );
            Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(cred.as_bytes())
            ))
        }
        None => None,
    };
    s.write_all(build_http_connect(host, port, auth_header.as_deref()).as_bytes())
        .await
        .map_err(|e| format!("代理请求失败: {e}"))?;
    let resp = read_http_headers(s).await?;
    parse_http_connect_response(&resp)
}

/// 经前置代理（或直连）建立到 host:port 的 TCP
pub async fn proxy_tcp(proxy: Option<&ProxyCfg>, host: &str, port: u16) -> Result<GenericStream, String> {
    let p = match proxy.filter(|p| p.enabled()) {
        None => {
            let t = TcpStream::connect((host, port))
                .await
                .map_err(|e| format!("连接 {host}:{port} 失败: {e}"))?;
            t.set_nodelay(true).ok();
            return Ok(Box::new(t));
        }
        Some(p) => p,
    };
    let mut t = TcpStream::connect((p.host.as_str(), p.port))
        .await
        .map_err(|e| format!("连接代理 {}:{} 失败: {e}", p.host, p.port))?;
    t.set_nodelay(true).ok();
    match p.kind.as_str() {
        "socks5" => socks5_handshake(&mut t, p, host, port).await?,
        "http" => http_connect_handshake(&mut t, p, host, port).await?,
        other => return Err(format!("未知代理类型: {other}")),
    }
    Ok(Box::new(t))
}

/* ---------- 跳板链隧道 ---------- */

fn hop_config() -> Arc<russh::client::Config> {
    Arc::new(russh::client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(10)),
        ..Default::default()
    })
}

/// 建立到 (host, port) 的隧道流：
/// hops 为空 → 前置代理直连；否则先建到第一级跳板的 SSH 会话（递归处理其自身
/// 的跳板/代理），认证后在其上开 direct-tcpip 通道到 (host, port)。
pub async fn tunnel_stream(
    hops: &[HopRec],
    proxy: Option<&ProxyCfg>,
    host: &str,
    port: u16,
) -> Result<GenericStream, String> {
    // async fn 递归需要装箱
    Box::pin(tunnel_stream_inner(hops, proxy, host, port)).await
}

async fn tunnel_stream_inner(
    hops: &[HopRec],
    proxy: Option<&ProxyCfg>,
    host: &str,
    port: u16,
) -> Result<GenericStream, String> {
    match hops.split_first() {
        None => proxy_tcp(proxy, host, port).await,
        Some((h, rest)) => {
            let up = tunnel_stream(rest, h.proxy.as_ref(), &h.host, h.port).await?;
            let handler = ClientHandler::new(None, h.password.clone());
            let mut handle: Handle<ClientHandler> =
                russh::client::connect_stream(hop_config(), up, handler)
                    .await
                    .map_err(|e| format!("SSH 连接失败(跳板 {}:{}): {e}", h.host, h.port))?;
            authenticate(
                &mut handle,
                &h.user,
                &h.password,
                h.private_key.as_deref(),
                h.passphrase.as_deref(),
            )
            .await
            .map_err(|e| format!("跳板 {}:{} 认证失败: {e}", h.host, h.port))?;
            let ch = handle
                .channel_open_direct_tcpip(host.to_string(), port as u32, "127.0.0.1".to_string(), 0)
                .await
                .map_err(|e| format!("跳板 {}:{} 开通道失败: {e}", h.host, h.port))?;
            Ok(Box::new(ch.into_stream()))
        }
    }
}

/// ssh.rs 连接入口：解析会话的代理/跳板配置，返回到目标 SSH 服务器的原始 TCP 流
pub async fn open_stream(
    config: &ConfigState,
    host: &str,
    port: u16,
    user: &str,
) -> Result<GenericStream, String> {
    let (proxy, hops) = resolve_network(config, host, port, user);
    tunnel_stream(&hops, proxy.as_ref(), host, port).await
}

/* ---------- 单元测试 ---------- */
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socks5_encoding() {
        assert_eq!(build_socks5_greeting(false), vec![5, 1, 0]);
        assert_eq!(build_socks5_greeting(true), vec![5, 1, 2]);
        assert_eq!(build_socks5_auth("u", "p"), vec![1, 1, b'u', 1, b'p']);
        assert_eq!(
            build_socks5_connect("example.com", 22),
            vec![5, 1, 0, 3, 11, b'e', b'x', b'a', b'm', b'p', b'l', b'e', b'.', b'c', b'o', b'm', 0, 22]
        );
    }

    #[test]
    fn http_connect_encoding_and_parse() {
        let req = build_http_connect("h", 22, Some("Basic AGE="));
        assert!(req.starts_with("CONNECT h:22 HTTP/1.1\r\n"));
        assert!(req.contains("Proxy-Authorization: Basic AGE=\r\n"));
        assert!(req.ends_with("\r\n\r\n"));
        assert!(parse_http_connect_response(b"HTTP/1.1 200 Connection established\r\n\r\n").is_ok());
        let e = parse_http_connect_response(b"HTTP/1.1 403 Denied\r\n\r\n").unwrap_err();
        assert!(e.contains("代理拒绝"));
        assert!(parse_http_connect_response(b"garbage").is_err());
    }

    #[test]
    fn proxy_cfg_enabled_semantics() {
        let p: ProxyCfg = serde_json::from_value(serde_json::json!({
            "kind": "socks5", "host": "127.0.0.1", "port": 1080,
            "username": "u", "password": "p"
        }))
        .unwrap();
        assert!(p.enabled());
        // 旧数据无 proxy 字段 → 默认构造即无代理（v4 兼容）
        let none = ProxyCfg::default();
        assert!(!none.enabled());
        let none_kind: ProxyCfg = serde_json::from_value(serde_json::json!({
            "kind": "", "host": "h", "port": 1
        }))
        .unwrap();
        assert!(!none_kind.enabled());
    }

    #[test]
    fn hop_depth_and_fields() {
        let rec = serde_json::json!({
            "host": "j1", "port": 2222, "username": "ops",
            "password": "pw", "privateKey": "C:/k",
            "proxy": { "kind": "http", "host": "corp", "port": 8080 },
            "jumpChain": ["x:1:a", "y:2:b"]
        });
        let h = hop_from_record(&rec);
        assert_eq!(h.host, "j1");
        assert_eq!(h.port, 2222);
        assert_eq!(h.user, "ops");
        assert!(h.proxy.as_ref().unwrap().enabled());
    }
}
