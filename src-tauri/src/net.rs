/* EasySSH v5(M2)/F-M2-05：SOCKS5 / HTTP CONNECT 前置代理 + ProxyJump 跳板链。
 * - 前置代理：会话记录 `proxy` 字段（none/socks5/http + 地址/端口/可选用户名密码），
 *   连接时先与代理握手，再经代理建立到 SSH 服务器的 TCP。
 * - ProxyJump：会话记录 `jumpChain` 字段 = 其他已存会话的 key 列表
 *   ["host:port:user", ...]（最多 3 级，超过后端显式报错拒绝连接）。连接链：
 *   逐级登录跳板机（复用其已存凭据与其自身代理/跳板），并在其上开
 *   direct-tcpip 通道到下一级，最终建立到目标机的 TCP 流。
 * - 代理/跳板均从 config.json 的会话数据解析（与 v4 sessions 兼容：
 *   旧数据无 proxy/jumpChain 字段 = 直连）。 */
use std::sync::Arc;

use russh::client::Handle;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::configstate::ConfigState;
use crate::gateway::{gateway_from_record, gateway_stream, GatewayCfg};
use crate::ssh::{authenticate, ClientHandler};
use crate::types::ProxyCfg;
// v5(beta) 任务二：握手/链路纯逻辑提取至 xfer_core（集成测试可编入）
use crate::xfer_core::{
    error_kind, hop_err_msg, jump_chain_overflow, record_matches, resolve_jump_chain,
    socks5_auth_reply, socks5_connect_reply_head, socks5_method_reply, SshErrKind,
};

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

/* ---------- 会话记录解析 ---------- */

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

/// 从 config 会话数组解析跳板链：目标会话记录里的 jumpChain 逐 key 查表展开。
/// 返回 (前置代理, 跳板链, 网关配置)；链深度超过 MAX_HOPS 时显式报错（不再静默截断，
/// 防自环/查表无命中剔除逻辑仍在 xfer_core::resolve_jump_chain）。
pub fn resolve_network(
    config: &ConfigState,
    host: &str,
    port: u16,
    user: &str,
) -> Result<(Option<ProxyCfg>, Vec<HopRec>, Option<GatewayCfg>), String> {
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
    let hops = match self_rec.and_then(|s| s.get("jumpChain")).and_then(|x| x.as_array()) {
        Some(chain) => {
            // v5.2.0：链超限先拒绝连接，防「连得通但链不完整」的静默截断
            if let Some((_, msg)) = jump_chain_overflow(chain, MAX_HOPS) {
                return Err(msg);
            }
            resolve_jump_chain(chain, &sessions, host, port, user, MAX_HOPS)
                .iter()
                .map(hop_from_record)
                .collect()
        }
        None => Vec::new(),
    };
    let gateway = self_rec.and_then(gateway_from_record);
    Ok((proxy, hops, gateway))
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

/// 解析 HTTP CONNECT 响应：实现移至 xfer_core（集成测试可测）
pub use crate::xfer_core::parse_http_connect_response;

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
    // v5(beta) 任务二：方法协商/认证子校验逻辑在 xfer_core（可测）
    let need_auth = socks5_method_reply(&hdr, with_auth)?;
    if need_auth {
        let u = proxy.username.as_deref().unwrap_or("");
        let p = proxy.password.as_deref().unwrap_or("");
        s.write_all(&build_socks5_auth(u, p))
            .await
            .map_err(|e| format!("代理认证失败: {e}"))?;
        let mut ar = [0u8; 2];
        s.read_exact(&mut ar).await.map_err(|e| format!("代理认证失败: {e}"))?;
        socks5_auth_reply(&ar)?;
    }
    s.write_all(&build_socks5_connect(host, port))
        .await
        .map_err(|e| format!("代理请求失败: {e}"))?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.map_err(|e| format!("代理请求失败: {e}"))?;
    let skip_fixed = socks5_connect_reply_head(&head)?;
    // 剩余变长地址 + 端口
    let skip = if head[3] == 0x03 {
        let mut l = [0u8; 1];
        s.read_exact(&mut l).await.map_err(|e| e.to_string())?;
        l[0] as usize + 2
    } else {
        skip_fixed
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
/// 每级失败报错带「跳板链第 N/M 跳」前缀与针对性建议（v5.2.0 链路诊断）。
pub async fn tunnel_stream(
    hops: &[HopRec],
    proxy: Option<&ProxyCfg>,
    host: &str,
    port: u16,
) -> Result<GenericStream, String> {
    let hop_total = hops.len();
    // async fn 递归需要装箱
    Box::pin(tunnel_stream_inner(hops, proxy, host, port, 0, hop_total)).await
}

/// 阶段推断：TCP 连接腿上的错误默认按 TCP 不通分类（Unknown 兜底同 TCP）；
/// SSH 握手腿（TCP 已建立后 connect_stream 失败）默认按握手失败分类。
fn hop_kind_at_tcp(e: &str) -> SshErrKind {
    match error_kind(e) {
        SshErrKind::TcpUnreachable | SshErrKind::Unknown => SshErrKind::TcpUnreachable,
        k => k,
    }
}

fn hop_kind_at_handshake(e: &str) -> SshErrKind {
    match error_kind(e) {
        SshErrKind::TcpUnreachable | SshErrKind::Unknown => SshErrKind::Handshake,
        k => k,
    }
}

async fn tunnel_stream_inner(
    hops: &[HopRec],
    proxy: Option<&ProxyCfg>,
    host: &str,
    port: u16,
    hop_index: usize, // 当前段在完整链中的 0 基下标（原始链绝对位置）
    hop_total: usize, // 完整链总跳数
) -> Result<GenericStream, String> {
    match hops.split_first() {
        None => {
            // hop_total == 0：目标直连（无跳板链），错误保持原有文案。
            // 否则本段 TCP 即第 hop_index+1 跳的连接腿，带跳序标注。
            proxy_tcp(proxy, host, port).await.map_err(|e| {
                if hop_total == 0 {
                    return e;
                }
                let k = hop_kind_at_tcp(&e);
                hop_err_msg(hop_index, hop_total, &format!("{host}:{port}"), &e, k)
            })
        }
        Some((h, rest)) => {
            let addr = format!("{}:{}", h.host, h.port);
            let up = Box::pin(tunnel_stream_inner(
                rest,
                h.proxy.as_ref(),
                &h.host,
                h.port,
                hop_index + 1,
                hop_total,
            ))
            .await?;
            let handler = ClientHandler::new(None, h.password.clone());
            let mut handle: Handle<ClientHandler> =
                russh::client::connect_stream(hop_config(), up, handler)
                    .await
                    .map_err(|e| {
                        let es = e.to_string();
                        let k = hop_kind_at_handshake(&es);
                        hop_err_msg(hop_index, hop_total, &addr, &es, k)
                    })?;
            authenticate(
                &mut handle,
                &h.user,
                &h.password,
                h.private_key.as_deref(),
                h.passphrase.as_deref(),
            )
            .await
            .map_err(|e| {
                hop_err_msg(hop_index, hop_total, &addr, &e, SshErrKind::Auth)
            })?;
            let ch = handle
                .channel_open_direct_tcpip(host.to_string(), port as u32, "127.0.0.1".to_string(), 0)
                .await
                .map_err(|e| {
                    let es = e.to_string();
                    let k = match error_kind(&es) {
                        SshErrKind::ChannelDenied | SshErrKind::Unknown => SshErrKind::ChannelDenied,
                        k => k,
                    };
                    hop_err_msg(hop_index, hop_total, &addr, &es, k)
                })?;
            Ok(Box::new(ch.into_stream()))
        }
    }
}

/// ssh.rs 连接入口：解析会话的代理/跳板/网关配置，返回到目标 SSH 服务器的原始 TCP 流。
/// v5.3/R-20261120-005(②)：会话带 gateway 配置时优先走网关流，
/// 主网关失败按 fallbacks 顺序转移；全部失败返回主网关错误（含分类错误码前缀）。
pub async fn open_stream(
    config: &ConfigState,
    host: &str,
    port: u16,
    user: &str,
) -> Result<GenericStream, String> {
    let (proxy, hops, gateway) = resolve_network(config, host, port, user)?;
    if let Some(gw) = gateway.filter(|g| g.enabled()) {
        let primary_err = match gateway_stream(&gw, host, port).await {
            Ok(s) => return Ok(s),
            Err(e) => e,
        };
        for fb in &gw.fallbacks {
            if !fb.enabled() {
                continue;
            }
            if let Ok(s) = gateway_stream(fb, host, port).await {
                return Ok(s);
            }
        }
        return Err(primary_err);
    }
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
