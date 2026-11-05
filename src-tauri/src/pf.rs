/* EasySSH v4：端口转发（对应 3.x pf:* 命令）
 * - 本地转发：监听 127.0.0.1:localPort，每条连接开 direct-tcpip 通道 → 目标主机:端口
 * - SOCKS 4/5 动态转发：本地 SOCKS 代理（SOCKS5 无认证 CONNECT，支持 IPv4/域名/IPv6；
 *   SOCKS4/4a CONNECT），目标由客户端请求指定
 * 生命周期：每个转发一个监听任务 + watch 停止信号；事件经 "pf:event" 推送。 */
use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::ssh::SessionMap;

#[derive(Clone)]
pub struct PfState {
    pub sessions: SessionMap,
    pub listeners: ArcListeners,
}

#[derive(Clone, Default)]
pub struct ArcListeners(
    pub std::sync::Arc<Mutex<HashMap<String, (tokio::sync::watch::Sender<bool>, serde_json::Value)>>>,
);

fn key_of(session_id: u64, local_port: u16) -> String {
    format!("{}:{}", session_id, local_port)
}

fn emit(app: &AppHandle, msg: String) {
    let _ = app.emit("pf:event", msg);
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PfConfig {
    #[serde(default)]
    pub name: Option<String>,
    pub local_port: u16,
    #[serde(default)]
    pub remote_host: Option<String>,
    #[serde(default)]
    pub remote_port: Option<u16>,
    /* 渲染端/已保存会话记录里该标志的历史字段名为 dynamic(见 ui/js/forward.js),
     * 此前 serde 静默丢弃导致 SOCKS 动态转发被当成普通本地转发启动(目标为空 →
     * 客户端握手前即被断开)。用 alias 兼容两种字段名,旧数据零迁移。 */
    #[serde(default, alias = "dynamic")]
    pub socks: bool,
}

/// 启动转发。返回监听键。
pub async fn start(
    state: PfState,
    app: AppHandle,
    session_id: u64,
    cfg: PfConfig,
) -> Result<String, String> {
    let key = key_of(session_id, cfg.local_port);
    let key_for_task = key.clone();
    if state.listeners.0.lock().contains_key(&key) {
        return Err(format!("本连接已在监听本地端口 {}", cfg.local_port));
    }
    let sess = state
        .sessions
        .0
        .lock()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| "会话不存在".to_string())?;

    let listener = TcpListener::bind(("127.0.0.1", cfg.local_port))
        .await
        .map_err(|e| format!("本地监听失败: {e}"))?;

    let (close_tx, mut close_rx) = tokio::sync::watch::channel(false);
    // 配置快照:pf_list 返回结构化条目(渲染端渲染运行中列表/停止用)
    let name = cfg.name.clone().unwrap_or_default();
    let snapshot = if cfg.socks {
        json!({ "name": name, "localPort": cfg.local_port, "socks": true, "dynamic": true, "running": true })
    } else {
        json!({
            "name": name,
            "localPort": cfg.local_port,
            "remoteHost": cfg.remote_host.clone().unwrap_or_default(),
            "remotePort": cfg.remote_port.unwrap_or(0),
            "socks": false, "dynamic": false, "running": true,
        })
    };
    state.listeners.0.lock().insert(key.clone(), (close_tx.clone(), snapshot.clone()));

    let st = state.clone();
    let app2 = app.clone();
    let socks = cfg.socks;
    let remote_host = cfg.remote_host.clone().unwrap_or_default();
    let remote_port = cfg.remote_port.unwrap_or(0);
    let lport = cfg.local_port;
    tokio::spawn(async move {
        emit(&app2, format!("[端口转发] 已启动 {}", key_for_task.clone()));
        loop {
            tokio::select! {
                _ = close_rx.changed() => {
                    if *close_rx.borrow() { break; }
                }
                accepted = listener.accept() => {
                    let Ok((client, peer)) = accepted else { break };
                    let st2 = st.clone();
                    let app3 = app2.clone();
                    let rh = remote_host.clone();
                    tokio::spawn(async move {
                        let _ = handle_client(st2, app3, session_id, socks, rh, remote_port, client, peer).await;
                    });
                }
                else => break,
            }
        }
        emit(&app2, format!("[端口转发] 已停止 {}", key_for_task.clone()));
        let _ = app2;
        let _ = lport;
    });

    Ok(key.clone())
}

/// 处理单条客户端连接：SOCKS 握手（可选）→ 打开 SSH 通道 → 双向拷贝
async fn handle_client(
    state: PfState,
    app: AppHandle,
    session_id: u64,
    socks: bool,
    fixed_host: String,
    fixed_port: u16,
    mut client: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
) -> Result<(), String> {
    let sess = state
        .sessions
        .0
        .lock()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| "连接不存在或已断开".to_string())?;

    let (target_host, target_port) = if socks {
        socks_handshake(&mut client).await?
    } else {
        (fixed_host.clone(), fixed_port)
    };

    let mut chan = sess
        .handle
        .lock()
        .await
        .channel_open_direct_tcpip(target_host.clone(), target_port as u32, peer.ip().to_string(), peer.port() as u32)
        .await
        .map_err(|e| format!("打开转发通道失败: {e}"))?;
    let mut stream = chan.into_stream();

    let _ = client.set_nodelay(true);
    let _ = app;
    tokio::io::copy_bidirectional(&mut client, &mut stream)
        .await
        .map_err(|e| format!("转发数据错误: {e}"))?;
    Ok(())
}

/// SOCKS 握手：支持 SOCKS5 CONNECT（无认证，IPv4/域名/IPv6）与 SOCKS4/4a CONNECT
async fn socks_handshake(client: &mut tokio::net::TcpStream) -> Result<(String, u16), String> {
    let mut b = [0u8; 2];
    client.read_exact(&mut b).await.map_err(|_| "SOCKS 读取失败".to_string())?;
    if b[0] == 0x04 {
        // SOCKS4 / 4a
        let cmd = b[1];
        if cmd != 1 {
            return Err("仅支持 SOCKS4 CONNECT 命令".into());
        }
        let mut rest = [0u8; 6];
        client.read_exact(&mut rest).await.map_err(|_| "SOCKS4 解析失败".to_string())?;
        let port = u16::from_be_bytes([rest[0], rest[1]]);
        let ip = [rest[2], rest[3], rest[4], rest[5]];
        // 4a: userid 后跟域名（IP 为 0.0.0.x 时）
        let mut uid = Vec::new();
        loop {
            let mut ch = [0u8; 1];
            client.read_exact(&mut ch).await.map_err(|_| "SOCKS4 解析失败".to_string())?;
            if ch[0] == 0 { break; }
            uid.push(ch[0]);
        }
        let host = if ip[0] == 0 && ip[1] == 0 && ip[2] == 0 && ip[3] != 0 {
            let mut host = Vec::new();
            loop {
                let mut ch = [0u8; 1];
                client.read_exact(&mut ch).await.map_err(|_| "SOCKS4a 解析失败".to_string())?;
                if ch[0] == 0 { break; }
                host.push(ch[0]);
            }
            String::from_utf8_lossy(&host).to_string()
        } else {
            format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
        };
        // 应答：VN=0, CD=90(同意)
        let _ = client.write_all(&[0x00, 0x5a, rest[0], rest[1], 0, 0, 0, 0]).await;
        return Ok((host, port));
    }

    // SOCKS5
    if b[0] != 0x05 {
        return Err("不支持的 SOCKS 版本".into());
    }
    let nmethods = b[1] as usize;
    let mut methods = vec![0u8; nmethods];
    client.read_exact(&mut methods).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
    if !methods.contains(&0x00) {
        // 仅支持无认证
        let _ = client.write_all(&[0x05, 0xff]).await;
        return Err("仅支持 SOCKS5 无认证".into());
    }
    let _ = client.write_all(&[0x05, 0x00]).await;

    let mut hdr = [0u8; 4];
    client.read_exact(&mut hdr).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
    if hdr[0] != 0x05 || hdr[1] != 1 {
        return Err("仅支持 SOCKS5 CONNECT 命令".into());
    }
    let host = match hdr[3] {
        1 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
            format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
            let mut host = vec![0u8; len[0] as usize];
            client.read_exact(&mut host).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
            String::from_utf8_lossy(&host).to_string()
        }
        4 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
            let mut parts = Vec::new();
            for seg in ip.chunks(2) {
                parts.push(format!("{:x}", u16::from_be_bytes([seg[0], seg[1]])));
            }
            parts.join(":")
        }
        t => return Err(format!("不支持的 SOCKS5 地址类型 {t}")),
    };
    let mut pt = [0u8; 2];
    client.read_exact(&mut pt).await.map_err(|_| "SOCKS5 解析失败".to_string())?;
    let port = u16::from_be_bytes(pt);
    // 应答：成功，绑定地址 0.0.0.0:0
    let _ = client
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await;
    Ok((host, port))
}

/// 停止指定转发
pub fn stop(state: &PfState, session_id: u64, local_port: u16) -> Result<(), String> {
    let key = key_of(session_id, local_port);
    let (tx, _) = state
        .listeners
        .0
        .lock()
        .remove(&key)
        .ok_or_else(|| "该端口没有正在运行的转发".to_string())?;
    let _ = tx.send(true);
    Ok(())
}

/// 某会话正在运行的转发列表
pub fn list(state: &PfState, session_id: u64) -> Vec<Value> {
    let prefix = format!("{}:", session_id);
    state
        .listeners
        .0
        .lock()
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| v.1.clone())
        .collect()
}
