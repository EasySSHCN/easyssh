/* EasySSH v5(beta) 任务二：xfer/net 纯逻辑提取模块（std + serde_json，零进程依赖）。
 * 目的：集成测试（src-tauri/tests/xfer_pipeline_tests.rs）可以像 update_guard 一样
 * 以 #[path] 把本模块直接编入测试 crate（binary crate 无 lib.rs），CI 全绿、无需 sshd。
 * 全部为纯函数/纯数据结构，行为与提取前完全一致：
 * - 传输管道：范围切分 split_ranges / 续传起点 resume_offsets
 * - 队列：优先级 rank、调度选路 pick_next、暂停状态机 worker_gate
 * - 限速（M4.1）：per_worker_bps / chunk_delay_ms
 * - 续传 journal：journal_parse（非法 JSON 安全回退空列表）/ journal_upsert_list
 * - 上传续传防改写：resume_upload_guard（远端比本地大 → 拒绝自动续传）
 * - SOCKS5 握手编解码（方法协商/认证子协商/CONNECT 应答）
 * - HTTP CONNECT 应答解析
 * - 跳板链展开（防自环去重 + MAX_HOPS 截断）
 * xfer.rs / net.rs 通过 pub use / 调用复用本模块，运行时行为不变。 */
use serde_json::Value;

/* ---------- 传输管道：范围切分与续传 ---------- */

/// 把 [start, end) 切成 n 个尽量均匀的连续区间
pub fn split_ranges(start: u64, end: u64, n: usize) -> Vec<(u64, u64)> {
    let n = n.max(1);
    let total = end.saturating_sub(start);
    if total == 0 {
        return Vec::new();
    }
    let n = n.min(total as usize);
    let base = total / n as u64;
    let rem = total % n as u64;
    let mut out = Vec::with_capacity(n);
    let mut cur = start;
    for i in 0..n {
        let len = base + if (i as u64) < rem { 1 } else { 0 };
        out.push((cur, cur + len));
        cur += len;
    }
    out
}

/// 续传起始偏移决策：
/// - 断点记录里的 rangeDone 与本次 worker 数一致 → 每个 worker 从各自断点继续
/// - 否则退化为不续传（offset = 区间起点，覆盖语义不变）
pub fn resume_offsets(range_done: Option<&[u64]>, ranges: &[(u64, u64)]) -> Vec<u64> {
    match range_done {
        Some(v) if v.len() == ranges.len() && !v.is_empty() => ranges
            .iter()
            .zip(v.iter())
            .map(|((s, e), d)| (*s + d).min(*e))
            .collect(),
        _ => ranges.iter().map(|(s, _)| *s).collect(),
    }
}

/* ---------- 队列：优先级 / 暂停状态机 ---------- */

/// 优先级字符串 → 调度 rank（越小越先）；未知值按 normal
pub fn priority_rank(p: &str) -> u8 {
    match p {
        "high" => 0,
        "low" => 2,
        _ => 1,
    }
}

/// 调度器选路：返回队列里 (rank, id) 最小者的下标（同 rank 先入队者先：id 小者优先）
pub fn pick_next(queue: &[(u8, u64)]) -> Option<usize> {
    if queue.is_empty() {
        return None;
    }
    let mut best = 0usize;
    for i in 1..queue.len() {
        if (queue[i].0, queue[i].1) < (queue[best].0, queue[best].1) {
            best = i;
        }
    }
    Some(best)
}

/// worker 每块间隙的门控决策（暂停状态机）
/// - 取消优先于暂停（取消时立即终止，不留挂起）
/// - 暂停时 WaitPaused：worker 不发起新块请求，在途块写完即停
#[derive(Debug, PartialEq, Eq)]
pub enum GateAction {
    Advance,
    WaitPaused,
    Cancelled,
}
pub fn worker_gate(paused: bool, cancelled: bool) -> GateAction {
    if cancelled {
        GateAction::Cancelled
    } else if paused {
        GateAction::WaitPaused
    } else {
        GateAction::Advance
    }
}

/* ---------- 限速（M4.1） ---------- */

/// 把总限速按 worker 数均分，得每 worker 字节/秒（speed_limit=0 → 0 = 不限速）
pub fn per_worker_bps(speed_limit: u64, workers: usize) -> u64 {
    if speed_limit == 0 {
        return 0;
    }
    let w = workers.max(1) as u64;
    (speed_limit / w).max(1)
}

/// 由限速推导每 worker 块间延迟（毫秒）；0 = 不限速；上限钳 60s 防极端小限速挂死
pub fn chunk_delay_ms(speed_limit: u64, workers: usize, chunk: u64) -> u64 {
    let per = per_worker_bps(speed_limit, workers);
    if per == 0 {
        return 0;
    }
    (chunk.saturating_mul(1000) / per).min(60_000)
}

/* ---------- 续传 journal（纯列表操作；磁盘读写留在 xfer.rs） ---------- */

/// 解析 journal 文本；文件不存在/损坏（非法 JSON）→ 空列表 → 安全走全量传输
pub fn journal_parse(txt: &str) -> Vec<Value> {
    serde_json::from_str(txt).unwrap_or_default()
}

/// journal upsert：按 (direction, localPath, remotePath) 去重替换；
/// received<=0（成功清理）时仅移除不追加。返回新列表。
pub fn journal_upsert_list(list: &[Value], task: &Value) -> Vec<Value> {
    let dir = task.get("direction").and_then(|x| x.as_str()).unwrap_or("");
    let lp = task.get("localPath").and_then(|x| x.as_str()).unwrap_or("");
    let rp = task.get("remotePath").and_then(|x| x.as_str()).unwrap_or("");
    let mut out: Vec<Value> = list
        .iter()
        .filter(|t| {
            !(t.get("direction").and_then(|x| x.as_str()) == Some(dir)
                && t.get("localPath").and_then(|x| x.as_str()) == Some(lp)
                && t.get("remotePath").and_then(|x| x.as_str()) == Some(rp))
        })
        .cloned()
        .collect();
    if task.get("received").and_then(|x| x.as_u64()).unwrap_or(0) > 0 {
        out.push(task.clone());
    }
    out
}

/* ---------- 上传续传防改写 ---------- */

/// 上传续传前校验：远端已有部分比本地源文件大 → 远端已被改写，拒绝自动续传
pub fn resume_upload_guard(remote_size: u64, total: u64) -> Result<(), String> {
    if remote_size > total {
        return Err("远端文件已变化(比本地大),建议重传".into());
    }
    Ok(())
}

/* ---------- SOCKS5 握手（字节流编解码） ---------- */

/// 方法协商应答校验：reply = [VER, METHOD]。
/// 返回 Ok(true)=需要继续用户名/密码子协商(0x02)，Ok(false)=无需认证(0x00)。
/// VER!=5 / 0xFF(无可接受方法) / 与请求不符 → Err。
pub fn socks5_method_reply(reply: &[u8], requested_auth: bool) -> Result<bool, String> {
    if reply.len() < 2 {
        return Err("代理握手失败: 应答过短".into());
    }
    if reply[0] != 0x05 {
        return Err("代理响应异常: 非 SOCKS5 协议".into());
    }
    match reply[1] {
        0x00 => {
            if requested_auth {
                Err("代理拒绝了认证方式".into())
            } else {
                Ok(false)
            }
        }
        0x02 => Ok(true),
        0xff => Err("代理拒绝了认证方式(不接受用户名密码认证)".into()),
        _ => Err("代理响应异常: 未知的认证方式".into()),
    }
}

/// 用户名/密码子协商应答校验：reply = [VER, STATUS]，STATUS==0 为成功
pub fn socks5_auth_reply(reply: &[u8]) -> Result<(), String> {
    if reply.len() < 2 {
        return Err("代理认证失败: 应答过短".into());
    }
    if reply[1] != 0x00 {
        return Err("代理认证失败: 用户名或密码被拒绝".into());
    }
    Ok(())
}

/// CONNECT 应答头校验（reply = [VER, REP, RSV, ATYP]）：
/// REP!=0 → 按码表给出中文原因；返回应答头之后剩余需读取的字节数（地址+端口）
pub fn socks5_connect_reply_head(reply: &[u8]) -> Result<usize, String> {
    if reply.len() < 4 {
        return Err("代理请求失败: 应答过短".into());
    }
    if reply[1] != 0x00 {
        let reason = match reply[1] {
            0x01 => "代理通用失败",
            0x02 => "代理规则不允许连接目标",
            0x03 => "目标网络不可达",
            0x04 => "目标主机不可达",
            0x05 => "代理连接目标被拒绝",
            0x06 => "TTL 过期",
            0x07 => "代理不支持该地址类型",
            _ => "代理连接失败",
        };
        return Err(format!("{reason} (SOCKS5 code {})", reply[1]));
    }
    let skip = match reply[3] {
        0x01 => 4 + 2,
        0x03 => 1 + 2, // 域名长度字节另读（调用方在剩余流上继续解析）
        0x04 => 16 + 2,
        _ => return Err("代理响应异常: 未知地址类型".into()),
    };
    Ok(skip)
}

/* ---------- HTTP CONNECT 应答解析 ---------- */

/// 解析 HTTP CONNECT 响应：非 200 → 区分「代理拒绝」与「响应异常」
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

/* ---------- 跳板链展开（防自环 + 截断） ---------- */

/// 两个会话记录是否同一目标（host:port:user 匹配；port 缺省 22）
pub fn record_matches(s: &Value, host: &str, port: u16, user: &str) -> bool {
    s.get("host").and_then(|x| x.as_str()) == Some(host)
        && s.get("port").and_then(|x| x.as_u64()).unwrap_or(22) as u16 == port
        && (rec_str(s, &["username", "user"]).as_deref() == Some(user))
}

fn rec_str(s: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(v) = s.get(*k).and_then(|x| x.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 从目标会话的 jumpChain（会话 key 列表）+ 会话表展开跳板链：
/// - key 形如 "host:port:user"，查表找不到的 key 跳过；
/// - 防自环：跳板不得指向目标自身；
/// - 链深度后端强制 ≤ max_hops。
/// 返回命中的会话记录（调用方再映射为 HopRec）。
pub fn resolve_jump_chain(
    chain: &[Value],
    sessions: &[Value],
    host: &str,
    port: u16,
    user: &str,
    max_hops: usize,
) -> Vec<Value> {
    let mut hops = Vec::new();
    for key in chain.iter().filter_map(|k| k.as_str()).take(max_hops) {
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
            hops.push(rec.clone());
        }
    }
    hops.truncate(max_hops);
    hops
}

/* ---------- 单元测试（模块级自检；集成测试另见 tests/xfer_pipeline_tests.rs） ---------- */
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socks5_method_reply_matrix() {
        assert_eq!(socks5_method_reply(&[5, 0], false), Ok(false));
        assert_eq!(socks5_method_reply(&[5, 2], true), Ok(true));
        assert!(socks5_method_reply(&[5, 0xff], true).is_err());
        assert!(socks5_method_reply(&[4, 0], false).is_err());
    }

    #[test]
    fn connect_reply_head_codes() {
        assert_eq!(socks5_connect_reply_head(&[5, 0, 0, 1]), Ok(6));
        assert_eq!(socks5_connect_reply_head(&[5, 0, 0, 3]), Ok(3));
        assert_eq!(socks5_connect_reply_head(&[5, 0, 0, 4]), Ok(18));
        assert!(socks5_connect_reply_head(&[5, 5, 0, 1]).is_err());
        assert!(socks5_connect_reply_head(&[5, 0, 0, 9]).is_err());
    }
}
