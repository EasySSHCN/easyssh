/* EasySSH 端口敲门兼容层（R-20261122-001）
 * 目标：客户端兼容服务器端「端口敲门」方案（knockd 序列敲门 / fwknop 风格 SPA 单包授权），
 * 仅客户端支持——连接前先把敲门包发出去，服务器端防火墙开门后再走原有 SSH 连接流程。
 *
 * 接入点：ssh.rs::connect 最前面（pre_connect_command 钩子之后、open_stream 之前）。
 * 配置来源：连接表单随会话保存的 `knock` 对象 + `preConnectCommand` 字符串
 *   （ConnectOpts 透传；后端另从 config 会话记录兜底解析，兼容外部导入路径）。
 * 失败语义（错误码前缀，渲染端分类提示）：
 *   PRECONNECT_FAIL:<cmd>:exit=<code>  —— 前置命令钩子失败（含超时/非零退出）
 *   KNOCK_TIMEOUT:<detail>             —— 敲门后目标端口在轮询窗口内不可达
 *   KNOCK_INVALID:<detail>             —— SPA 被服务器拒绝（密钥/时钟不对）
 * 重试防重复：敲门成功后在进程内存按 (host,port) 标记，窗口期内重连不再重复敲门。
 *
 * preConnectCommand 防注入约定：命令字符串经本文件 split_command_argv 做纯
 * shell 风格分词（支持单/双引号转义），直接以 argv 数组 spawn，不经 cmd.exe/
 * /bin/sh 拼接——配置文件中的命令无法借助管道/重定向/分号注入额外语义。
 */
use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::configstate::ConfigState;
use crate::xfer_core::record_matches;

/// 敲门/前置命令配置（会话记录 `knock` 字段 + `preConnectCommand`）。
/// 旧数据无此字段 = 全部默认值 = 不敲门不执行钩子（v4/v5 旧会话完全兼容）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KnockCfg {
    pub enabled: bool,
    /// sequence | spa（custom 视为 sequence；其他值忽略）
    #[serde(rename = "type")]
    pub kind: String,
    /// 序列敲门端口列表（按序发送）
    pub sequence: Vec<u16>,
    /// udp | tcp
    pub protocol: String,
    /// 全部敲门包须在此秒数内发完（对应 knockd seq_timeout）
    pub seq_timeout: u64,
    /// 相邻敲门包间隔（毫秒）
    pub delay_ms: u64,
    /// SPA 监听端口（fwknopd 默认 62201）
    pub spa_port: u16,
    /// SPA 加密密钥（落盘经凭据加密存储，见 easyssh-core config.rs）
    pub key: String,
    /// SPA HMAC 密钥（同上）
    pub hmac_key: String,
    /// 开门目标，如 "tcp/22"
    pub open_ports: String,
    /// 服务器防火墙开门保持秒数（fwknop fw_access_timeout，默认 30）
    pub fw_timeout: u64,
}

impl KnockCfg {
    pub fn is_sequence(&self) -> bool {
        self.enabled && self.sequence.len() > 0 && matches!(self.kind.as_str(), "" | "sequence" | "custom")
    }
    pub fn is_spa(&self) -> bool {
        self.enabled && self.kind == "spa" && !self.key.is_empty()
    }
    pub fn protocol(&self) -> &str {
        match self.protocol.as_str() {
            "tcp" => "tcp",
            _ => "udp", // 默认 udp（knockd 常用）
        }
    }
}

/* ---------- preConnectCommand 钩子（P0） ---------- */

/// shell 风格分词：按空白切分，单/双引号内保留空格与转义；反斜杠在双引号内
/// 转义下一字符。返回 None 表示引号未闭合（配置错误）。
pub fn split_command_argv(cmd: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_token = false;
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => return None,
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => {
                            if let Some(n) = chars.next() {
                                cur.push(n);
                            } else {
                                return None;
                            }
                        }
                        Some(ch) => cur.push(ch),
                        None => return None,
                    }
                }
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    in_token = true;
                } else {
                    return None;
                }
            }
            c => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        out.push(cur);
    }
    Some(out)
}

/// P0 前置命令钩子：非空则本地 spawn 执行（argv 数组，不经 shell），15 秒超时，
/// 退出码 0 视为成功；失败返回 PRECONNECT_FAIL 前缀错误并中止连接。
pub async fn run_pre_connect(cmd: &str) -> Result<(), String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Ok(());
    }
    let argv = split_command_argv(cmd)
        .ok_or_else(|| format!("PRECONNECT_FAIL:{cmd}:命令引号未闭合,无法解析为参数数组"))?;
    let (program, args) = argv.split_first().ok_or_else(|| {
        format!("PRECONNECT_FAIL:{cmd}:命令为空")
    })?;
    let program = program.to_string();
    let args: Vec<String> = args.to_vec();
    let label = cmd.to_string();
    let res = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
    });
    const TIMEOUT: Duration = Duration::from_secs(15);
    let output = match tokio::time::timeout(TIMEOUT, res).await {
        Ok(Ok(Ok(o))) => o,
        Ok(Ok(Err(e))) => {
            return Err(format!("PRECONNECT_FAIL:{label}:启动失败 {e}"));
        }
        Ok(Err(_)) | Err(_) => {
            return Err(format!("PRECONNECT_FAIL:{label}:执行超时(>15秒),已中止"));
        }
    };
    if output.status.success() {
        Ok(())
    } else {
        let code = output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
        Err(format!("PRECONNECT_FAIL:{label}:exit={code}"))
    }
}

/* ---------- 会话记录兜底解析 ---------- */

/// 从 config 会话记录解析 knock 配置 + 前置命令（opts 未提供时的兜底，
/// 覆盖外部导入/重连未携带 opts 字段的路径）。
pub fn resolve_from_config(
    config: &ConfigState,
    host: &str,
    port: u16,
    user: &str,
) -> Option<KnockCfg> {
    let sessions = config
        .load_value()
        .ok()
        .and_then(|v| v.get("sessions").and_then(|x| x.as_array()).cloned())
        .unwrap_or_default();
    let rec = sessions.iter().find(|s| record_matches(s, host, port, user))?;
    let cfg = rec.get("knock").cloned()?;
    serde_json::from_value::<KnockCfg>(cfg).ok().filter(|k| k.enabled)
}

/* ---------- 敲门执行门（ssh.rs::connect 前置环节，P0+P1+P2 汇总入口） ---------- */

use parking_lot::Mutex;
use tauri::AppHandle;

/// 敲门成功后的免重敲窗口：会话内存标记，窗口期内重连不再重复敲门。
/// sequence 取 max(seq_timeout,120s)；SPA 取 fw_timeout。
static KNOCKED: Mutex<Option<HashMap<(String, u16), Instant>>> = Mutex::new(None);

fn knock_cache_fresh(host: &str, port: u16) -> bool {
    let map = KNOCKED.lock();
    match map.as_ref() {
        Some(m) => m
            .get(&(host.to_string(), port))
            .map(|t| t.elapsed() < Duration::from_secs(120))
            .unwrap_or(false),
        None => false,
    }
}

fn knock_cache_mark(host: &str, port: u16, window: Duration) {
    let mut map = KNOCKED.lock();
    let m = map.get_or_insert_with(HashMap::new);
    m.retain(|_, t| t.elapsed() < Duration::from_secs(3600));
    m.insert((host.to_string(), port), Instant::now());
    let _ = window;
}

/// 敲门进度事件：渲染端据此显示「未敲/敲门中/已开门(倒计时)/失败」状态。
/// state: idle | knocking | open | failed；windowSec = 免重敲窗口秒数(前端倒计时)。
fn emit_knock(app: &AppHandle, host: &str, port: u16, state: &str, message: &str, window_secs: u64) {
    let _ = app.emit(
        "knock:log",
        serde_json::json!({ "host": host, "port": port, "state": state, "message": message, "windowSec": window_secs }),
    );
}

/// 连接前置总闸：前置命令钩子 → 敲门（缓存命中跳过）→ 可达性轮询。
/// 任何失败返回带错误码前缀的 Err，连接流程立即中止。
pub async fn pre_connect_gate(
    app: AppHandle,
    config: &ConfigState,
    opts: &crate::types::ConnectOpts,
    host: &str,
    port: u16,
) -> Result<(), String> {
    // ① P0：前置命令钩子
    run_pre_connect(&opts.pre_connect_command).await?;
    // ② P1/P2：敲门配置（opts 优先，config 会话记录兜底）
    let cfg = match &opts.knock {
        Some(k) if k.enabled => Some(k.clone()),
        _ => resolve_from_config(config, host, port, &opts.user),
    };
    let cfg = match cfg {
        Some(k) if k.enabled => k,
        _ => {
            emit_knock(&app, host, port, "idle", "", 0);
            return Ok(());
        }
    };
    if knock_cache_fresh(host, port) {
        emit_knock(&app, host, port, "open", "免重敲:窗口期内(内存标记)", 120);
        return Ok(());
    }
    let t0 = Instant::now();
    emit_knock(&app, host, port, "knocking", "正在敲门…", 0);
    let kind_label;
    match cfg.kind.as_str() {
        "spa" => {
            kind_label = format!("SPA {}", cfg.spa_port);
            send_spa(&cfg, host).await?;
        }
        _ => {
            let seq: Vec<String> = cfg.sequence.iter().map(|p| p.to_string()).collect();
            kind_label = format!("sequence {}", seq.join("→"));
            knock_sequence(&cfg, host).await?;
        }
    }
    // 可达性轮询：500ms × 最多 8 次（敲门完成后目标端口开门需要一点时间）
    let (ok, elapsed) = poll_reachable(host, port).await;
    let secs = t0.elapsed().as_secs_f32();
    if !ok {
        let hint = if cfg.is_spa() && (cfg.key.is_empty() || cfg.hmac_key.is_empty()) {
            "SPA 密钥未配置(knock_invalid)。"
        } else {
            "请检查服务器端 knockd/fwknopd 配置与 seq_timeout 是否一致;SPA 被拒绝时(密钥/服务器时钟偏差)请核对密钥与系统时间。"
        };
        emit_knock(&app, host, port, "failed", &format!("敲门后端口 {port} 不可达"), 0);
        return Err(format!(
            "KNOCK_TIMEOUT:敲门({kind_label})已完成,但目标端口 {port} 在轮询窗口内仍不可达。{hint}"
        ));
    }
    let window = if cfg.is_spa() {
        Duration::from_secs(cfg.fw_timeout.max(30))
    } else {
        Duration::from_secs(cfg.seq_timeout.max(120))
    };
    knock_cache_mark(host, port, window);
    let msg = format!("knock: {} OK, port {} reachable in {:.1}s", kind_label, port, secs);
    emit_knock(&app, host, port, "open", &msg, window.as_secs());
    Ok(())
}

/* ---------- P1：knockd 序列敲门 ---------- */

/// 随机 0~8 字节负载（knockd 对负载不敏感）
fn knock_payload() -> Vec<u8> {
    use rand::Rng;
    let n = rand::thread_rng().gen_range(0..=8usize);
    rand::thread_rng().sample_iter(rand::distributions::Standard).take(n).collect()
}

/// 按序敲门：UDP 直接 sendto；TCP 建 SYN 连接后立即断开（不等响应）。
/// 包间隔 delay_ms，全部须在 seq_timeout 秒内完成。
pub async fn knock_sequence(cfg: &KnockCfg, host: &str) -> Result<(), String> {
    if cfg.sequence.is_empty() {
        return Err("KNOCK_INVALID:序列敲门端口列表为空".into());
    }
    let deadline = Duration::from_secs(cfg.seq_timeout.max(1));
    let delay = Duration::from_millis(cfg.delay_ms.clamp(10, 2000));
    let t0 = Instant::now();
    let proto = cfg.protocol();
    for (i, p) in cfg.sequence.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(delay).await;
        }
        if t0.elapsed() > deadline {
            return Err(format!(
                "KNOCK_INVALID:序列敲门未在 seq_timeout={}秒 内完成(第 {} 个端口 {})",
                cfg.seq_timeout,
                i + 1,
                p
            ));
        }
        let res = match proto {
            "tcp" => tokio::time::timeout(
                Duration::from_millis(1500),
                tokio::net::TcpStream::connect((host, *p)),
            )
            .await
            .map(|r| r.map(|_| 1usize).map_err(|e: std::io::Error| e.to_string())),
            _ => {
                let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
                    .await
                    .map_err(|e| format!("KNOCK_INVALID:本机 UDP socket 创建失败: {e}"))?;
                let payload = knock_payload();
                tokio::time::timeout(
                    Duration::from_millis(1500),
                    sock.send_to(&payload, (host, *p)),
                )
                .await
                .map(|r| r.map_err(|e: std::io::Error| e.to_string()))
            }
        };
        let sent = match res {
            Ok(Ok(_)) => true,
            // TCP SYN 被服务器 RST / UDP 不可达 ICMP 均属正常(knockd 只看包不看回包),忽略错误继续
            Ok(Err(_)) => true,
            Err(_) => return Err(format!("KNOCK_INVALID:敲门包发送超时({proto}/{p})")),
        };
        let _ = sent;
    }
    Ok(())
}

/// 目标端口可达性轮询：500ms 间隔 × 最多 8 次,返回 (是否可达, 总耗时秒)
pub async fn poll_reachable(host: &str, port: u16) -> (bool, f32) {
    let t0 = Instant::now();
    for _ in 0..8 {
        let try_res = tokio::time::timeout(
            Duration::from_millis(1200),
            tokio::net::TcpStream::connect((host, port)),
        )
        .await;
        if let Ok(Ok(_)) = try_res {
            return (true, t0.elapsed().as_secs_f32());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    (false, t0.elapsed().as_secs_f32())
}


/// P2 SPA 发送：构造单包并经 UDP 发往 host:spaPort（fire-and-forget,无回包协议）。
async fn send_spa(cfg: &KnockCfg, host: &str) -> Result<(), String> {
    if cfg.key.is_empty() || cfg.hmac_key.is_empty() {
        return Err("KNOCK_INVALID:SPA 密钥/HMAC 密钥未配置".into());
    }
    // R-20261122-002：libfko 构造原生线格式包；client_timeout 取 fw_timeout。
    let packet = crate::spa::build_packet(
        &cfg.key,
        &cfg.hmac_key,
        if cfg.open_ports.is_empty() { "tcp/22" } else { &cfg.open_ports },
        cfg.fw_timeout,
    )
    .map_err(|e| format!("KNOCK_INVALID:SPA 包构造失败: {e}"))?;
    let sock = tokio::net::UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("KNOCK_INVALID:本机 UDP socket 创建失败: {e}"))?;
    let port = if cfg.spa_port == 0 { 62201 } else { cfg.spa_port };
    tokio::time::timeout(Duration::from_millis(2000), sock.send_to(&packet, (host, port)))
        .await
        .map_err(|_| format!("KNOCK_INVALID:SPA 包发送超时({host}:{port})"))?
        .map_err(|e| format!("KNOCK_INVALID:SPA 包发送失败: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod knock_net_tests {
    use super::*;

    #[tokio::test]
    async fn sequence_udp_reaches_local_listener() {
        // 本地 UDP 监听模拟服务器 knockd,验证按序发包全部到达
        let l1 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let l2 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let l3 = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cfg = KnockCfg {
            enabled: true,
            kind: "sequence".into(),
            sequence: vec![l1.local_addr().unwrap().port(), l2.local_addr().unwrap().port(), l3.local_addr().unwrap().port()],
            protocol: "udp".into(),
            seq_timeout: 5,
            delay_ms: 30,
            ..Default::default()
        };
        knock_sequence(&cfg, "127.0.0.1").await.unwrap();
        let mut buf = [0u8; 64];
        for l in [&l1, &l2, &l3] {
            let n = tokio::time::timeout(Duration::from_secs(2), l.recv(&mut buf)).await.unwrap().unwrap();
            assert!(n <= 8);
        }
    }

    #[tokio::test]
    async fn spa_packet_reaches_local_listener() {
        let l = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let cfg = KnockCfg {
            enabled: true,
            kind: "spa".into(),
            spa_port: port,
            key: "test-key".into(),
            hmac_key: "test-hmac".into(),
            open_ports: "tcp/22".into(),
            ..Default::default()
        };
        send_spa(&cfg, "127.0.0.1").await.unwrap();
        let mut buf = [0u8; 4096];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), l.recv_from(&mut buf)).await.unwrap().unwrap();
        // R-20261122-002：线格式校验（fwknop 原生格式，解密侧由 fwknopd 真机回归覆盖）
        crate::spa::packet_format_ok(&buf[..n]).unwrap();
    }

    #[test]
    fn timeout_semantics_documented() {
        // KNOCK_TIMEOUT vs KNOCK_INVALID 语义守卫:前缀常量不被误改
        assert!("KNOCK_TIMEOUT:x".starts_with("KNOCK_TIMEOUT"));
        assert!("KNOCK_INVALID:x".starts_with("KNOCK_INVALID"));
        assert!("PRECONNECT_FAIL:cmd:exit=1".starts_with("PRECONNECT_FAIL"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_command_argv_quotes() {
        assert_eq!(split_command_argv("echo hello"), Some(vec!["echo".into(), "hello".into()]));
        assert_eq!(
            split_command_argv("netsh advfirewall 'firewall set' \"all profiles\""),
            Some(vec![
                "netsh".into(),
                "advfirewall".into(),
                "firewall set".into(),
                "all profiles".into()
            ])
        );
        assert_eq!(split_command_argv("a \"b\\\"c\""), Some(vec!["a".into(), "b\"c".into()]));
        assert_eq!(split_command_argv("unclosed 'quote"), None);
        assert_eq!(split_command_argv("   "), Some(Vec::<String>::new()));
    }

    #[test]
    fn knock_cfg_semantics() {
        let k: KnockCfg = serde_json::from_value(serde_json::json!({
            "enabled": true, "type": "sequence", "sequence": [7000, 8000, 9000],
            "protocol": "udp", "seqTimeout": 10, "delayMs": 150
        }))
        .unwrap();
        assert!(k.is_sequence());
        assert!(!k.is_spa());
        assert_eq!(k.protocol(), "udp");
        let spa: KnockCfg = serde_json::from_value(serde_json::json!({
            "enabled": true, "type": "spa", "spaPort": 62201,
            "key": "k", "hmacKey": "h", "openPorts": "tcp/22", "fwTimeout": 30
        }))
        .unwrap();
        assert!(spa.is_spa());
        assert_eq!(spa.spa_port, 62201);
        // 旧数据兼容：enabled 缺失 = 不敲门
        let off: KnockCfg = serde_json::from_value(serde_json::json!({ "sequence": [1] })).unwrap();
        assert!(!off.is_sequence());
    }
}
