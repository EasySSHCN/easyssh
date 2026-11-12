/* EasySSH v5.1.1/R-20261111-004：IP 归属地主备查询
 *
 * 查询链（与官网服务端 st_region_name() 同规格）：
 *   主 https://ip.263.tw/api/v1/ 加密归属地 API（IPv4/IPv6 兼容）：
 *     ① GET /ip?q=<IP>（省略=查自身，Authorization: Bearer <token>）
 *        → AES-256-GCM 载荷 base64url(IV(12B)|密文|Tag(16B))
 *     ② POST /decrypt payload=<载荷> → JSON { ip, country, region, prov, city, isp }
 *   备① 百度 qifu（v4 专用，实测 v6 返回空 data:[]）
 *   备② ip-api.com（lang=zh-CN，免费档仅 HTTP、限频 45 次/分，只作末级兜底）
 *
 * 行为要求：
 *   - 主接口超时(3s)/非 200/解密失败 任一情况自动切备用；
 *   - 两者都失败时静默降级（返回 Err，前端显示暂无归属地，不弹错误）；
 *   - 保留/内网地址不外呼不缓存（直接返回分类结果）；
 *   - 结果本地缓存按 IP 前缀（IPv4 前两段 / IPv6 前两个 hextet），TTL 7 天；
 *   - token 经设置 settings.geoToken 或环境变量 EASYSSH_GEO_TOKEN 注入，
 *     支持用户自备，禁止硬编码进客户端或公开仓库；
 *   - 兼容 IPv6：主/备② 均支持 v4+v6，避免 v6 场景静默失败。
 */
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const PRIMARY_IP_URL: &str = "https://ip.263.tw/api/v1/ip";
const PRIMARY_DECRYPT_URL: &str = "https://ip.263.tw/api/v1/decrypt";
const BAIDU_URL: &str = "https://qifu-api.baidubce.com/ip/local/geo/v1/district";
const IPAPI_URL: &str = "http://ip-api.com/json";
const CACHE_FILE: &str = "geo-cache.json";
const CACHE_TTL_MS: u64 = 7 * 24 * 3600 * 1000;

/* ---------------- IP 形态识别 ---------------- */

pub(crate) fn parse_ipv4(ip: &str) -> Option<[u8; 4]> {
    let seg: Vec<&str> = ip.split('.').collect();
    if seg.len() != 4 { return None; }
    let mut o = [0u8; 4];
    for (i, s) in seg.iter().enumerate() {
        o[i] = s.parse::<u8>().ok()?;
    }
    Some(o)
}

pub(crate) fn is_ipv6(ip: &str) -> bool {
    ip.contains(':')
        && ip.len() >= 2
        && ip.len() <= 45
        && ip.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
}

/// 保留/内网地址：不外呼不缓存
fn is_reserved(ip: &str) -> bool {
    if let Some(o) = parse_ipv4(ip) {
        return o[0] == 0 || o[0] == 10 || o[0] == 127
            || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT
            || (o[0] == 169 && o[1] == 254)                // 链路本地
            || (o[0] == 172 && (16..=31).contains(&o[1]))  // 私网
            || (o[0] == 192 && o[1] == 168)                // 私网
            || (o[0] == 198 && (18..=19).contains(&o[1])); // 文档
    }
    if is_ipv6(ip) {
        let s = ip.to_ascii_lowercase();
        return s == "::" || s == "::1"
            || s.starts_with("fe8") || s.starts_with("fe9") || s.starts_with("fea") || s.starts_with("feb") // fe80::/10
            || s.starts_with("fc") || s.starts_with("fd")                                                    // ULA fc00::/7
            || s.starts_with("2001:db8"); // 文档段
    }
    false
}

fn reserved_result(ip: &str) -> Value {
    let loopback = ip == "::1" || ip == "127.0.0.1";
    json!({
        "ip": ip,
        "country": "保留地址",
        "region": "", "prov": "", "city": "",
        "isp": if loopback { "内网/环回/保留地址" } else { "内网·保留地址" },
        "source": "local",
    })
}

/// 缓存键：IPv4 前两段 / IPv6 前两个 hextet（v6 空 hextet 视为 "0"）
fn prefix_key(ip: &str) -> Option<String> {
    if let Some(o) = parse_ipv4(ip) {
        return Some(format!("{}.{}", o[0], o[1]));
    }
    if is_ipv6(ip) {
        let h: Vec<&str> = ip.split(':').filter(|s| !s.is_empty()).collect();
        if h.len() >= 2 {
            return Some(format!("{}:{}", h[0].to_ascii_lowercase(), h[1].to_ascii_lowercase()));
        }
    }
    None
}

/* ---------------- token 来源（设置 geoToken 优先，其次环境变量） ---------------- */

fn geo_token(base: &Path) -> String {
    if let Ok(c) = easyssh_core::config::ConfigStore::new(base.to_path_buf()).load() {
        if let Some(t) = c.settings.get("geoToken").and_then(|x| x.as_str()) {
            let t = t.trim().to_string();
            if !t.is_empty() { return t; }
        }
    }
    std::env::var("EASYSSH_GEO_TOKEN").unwrap_or_default().trim().to_string()
}

/* ---------------- 本地缓存（exe 同目录 geo-cache.json） ---------------- */

fn cache_load(base: &Path) -> Value {
    std::fs::read_to_string(base.join(CACHE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({}))
}

fn cache_get(base: &Path, key: &str) -> Option<Value> {
    let now = now_ms();
    cache_load(base).get(key).and_then(|v| {
        let ts = v.get("ts").and_then(|x| x.as_u64()).unwrap_or(0);
        if ts != 0 && now.saturating_sub(ts) < CACHE_TTL_MS {
            v.get("result").cloned()
        } else {
            None
        }
    })
}

fn cache_put(base: &Path, key: &str, result: &Value) {
    let mut c = cache_load(base);
    if let Some(o) = c.as_object_mut() {
        // 防失控：缓存条目上限 512，超出时丢弃最旧的一半
        if o.len() >= 512 {
            let mut keys: Vec<(String, u64)> = o.iter()
                .map(|(k, v)| (k.clone(), v.get("ts").and_then(|x| x.as_u64()).unwrap_or(0)))
                .collect();
            keys.sort_by_key(|(_, ts)| *ts);
            for (k, _) in keys.into_iter().take(256) { o.remove(&k); }
        }
        o.insert(key.to_string(), json!({ "ts": now_ms(), "result": result }));
    }
    let _ = std::fs::write(base.join(CACHE_FILE), c.to_string());
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/* ---------------- 查询实现（阻塞 HTTP 放线程池） ---------------- */

fn http_get(url: &str, token: &str, timeout_s: u64) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_s))
        .build();
    let mut req = agent.get(url);
    if !token.is_empty() {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let resp = req.call().map_err(|e| format!("请求失败: {e}"))?;
    resp.into_string().map_err(|e| format!("读取响应失败: {e}"))
}

fn http_post_form(url: &str, body: &str, token: &str, timeout_s: u64) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(timeout_s))
        .build();
    let mut req = agent.post(url)
        .set("Content-Type", "application/x-www-form-urlencoded");
    if !token.is_empty() {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let resp = req.send_string(body).map_err(|e| format!("请求失败: {e}"))?;
    resp.into_string().map_err(|e| format!("读取响应失败: {e}"))
}

fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 主接口：/ip 取加密载荷 → /decrypt 解密得 JSON 原文
fn primary_query(ip: Option<&str>, token: &str) -> Result<Value, String> {
    if token.is_empty() {
        return Err("主接口未配置 token(设置 geoToken 或环境变量 EASYSSH_GEO_TOKEN)".into());
    }
    let url = match ip {
        Some(q) => format!("{PRIMARY_IP_URL}?q={}", form_encode(q)),
        None => PRIMARY_IP_URL.to_string(),
    };
    let payload = http_get(&url, token, 3)?;
    let payload = payload.trim().trim_matches('"').to_string();
    if payload.is_empty() {
        return Err("主接口返回空载荷".into());
    }
    let body = http_post_form(PRIMARY_DECRYPT_URL, &format!("payload={}", form_encode(&payload)), token, 3)?;
    let j: Value = serde_json::from_str(body.trim()).map_err(|e| format!("主接口解密响应解析失败: {e}"))?;
    if j.get("country").and_then(|x| x.as_str()).is_none() {
        return Err("主接口响应缺少 country 字段".into());
    }
    Ok(j)
}

/// 备① 百度 qifu（v4 专用）
fn baidu_query(ip: Option<&str>) -> Result<Value, String> {
    let v4 = match ip {
        Some(q) => parse_ipv4(q).ok_or("百度备用接口仅支持 IPv4")?,
        None => return Err("百度备用接口不支持查自身(需显式 IPv4)".into()),
    };
    let url = format!("{BAIDU_URL}?ip={}.{}.{}.{}", v4[0], v4[1], v4[2], v4[3]);
    let body = http_get(&url, "", 5)?;
    let j: Value = serde_json::from_str(body.trim()).map_err(|e| format!("百度备用响应解析失败: {e}"))?;
    if j.get("code").and_then(|x| x.as_str()) != Some("Success") {
        return Err(format!("百度备用返回异常: {}", j.get("code").and_then(|x| x.as_str()).unwrap_or("未知")));
    }
    let d = j.get("data").ok_or("百度备用响应缺少 data")?;
    let prov = d.get("prov").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let city = d.get("city").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let isp = d.get("isp").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    if prov.is_empty() && city.is_empty() {
        return Err("百度备用无归属地数据".into());
    }
    Ok(json!({
        "ip": d.get("ip").and_then(|x| x.as_str()).unwrap_or(""),
        "country": "中国", "region": "", "prov": prov, "city": city, "isp": isp,
        "source": "baidu",
    }))
}

/// 备② ip-api.com（v4+v6，中文，免费档仅 HTTP）
fn ipapi_query(ip: Option<&str>) -> Result<Value, String> {
    let url = match ip {
        Some(q) => format!("{IPAPI_URL}/{}?lang=zh-CN&fields=status,message,country,regionName,city,isp,query", form_encode(q)),
        None => format!("{IPAPI_URL}?lang=zh-CN&fields=status,message,country,regionName,city,isp,query"),
    };
    let body = http_get(&url, "", 5)?;
    let j: Value = serde_json::from_str(body.trim()).map_err(|e| format!("ip-api 备用响应解析失败: {e}"))?;
    if j.get("status").and_then(|x| x.as_str()) != Some("success") {
        return Err(format!("ip-api 备用返回异常: {}", j.get("message").and_then(|x| x.as_str()).unwrap_or("未知")));
    }
    Ok(json!({
        "ip": j.get("query").and_then(|x| x.as_str()).unwrap_or(""),
        "country": j.get("country").and_then(|x| x.as_str()).unwrap_or(""),
        "region": "", "prov": j.get("regionName").and_then(|x| x.as_str()).unwrap_or(""),
        "city": j.get("city").and_then(|x| x.as_str()).unwrap_or(""),
        "isp": j.get("isp").and_then(|x| x.as_str()).unwrap_or(""),
        "source": "ip-api",
    }))
}

/* ---------------- 对外入口 ---------------- */
/// 主备查询链入口：保留地址不外呼；命中缓存直接返回；否则 主→备①→备② 依次降级。
async fn lookup_inner(base: &Path, ip: Option<String>) -> Result<Value, String> {
    let ip = ip.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(ref q) = ip {
        if parse_ipv4(q).is_none() && !is_ipv6(q) {
            return Err(format!("无效的 IP 地址: {q}"));
        }
        if is_reserved(q) {
            return Ok(reserved_result(q));
        }
        if let Some(key) = prefix_key(q) {
            if let Some(hit) = cache_get(base, &key) {
                return Ok(hit);
            }
        }
    }

    let token = geo_token(base);
    let q = ip.as_deref();
    // 阻塞 HTTP 放线程池（与 feedback::post_envelope 同模式）
    let (result, primary_err) = {
        let token = token.clone();
        let q = q.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let mut primary_err = None;
            let mut result = match primary_query(q.as_deref(), &token) {
                Ok(v) => Some(v),
                Err(e) => { primary_err = Some(e.to_string()); None }
            };
            // 主接口失败（超时/非200/解密失败）→ 备① 百度(v4) → 备② ip-api.com(v4+v6)
            if result.is_none() {
                result = baidu_query(q.as_deref()).ok().or_else(|| ipapi_query(q.as_deref()).ok());
            }
            (result, primary_err)
        })
        .await
        .map_err(|e| format!("任务错误: {e}"))?
    };

    let mut result = result.ok_or_else(|| {
        format!("归属地查询失败({});已静默降级", primary_err.unwrap_or_else(|| "所有接口不可用".into()))
    })?;
    // 主接口响应无 source 字段,补标记便于前端展示来源
    if let Some(o) = result.as_object_mut() {
        o.entry("source").or_insert(json!("ip.263.tw"));
    }

    // 缓存按前缀（查自身时无法确定前缀，不缓存）
    if let Some(ref q) = ip {
        if let Some(key) = prefix_key(q) {
            cache_put(base, &key, &result);
        }
    }
    Ok(result)
}

pub(crate) async fn lookup(base: &PathBuf, ip: Option<String>) -> Result<Value, String> {
    lookup_inner(base, ip).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_reserved() {
        assert_eq!(parse_ipv4("1.2.3.4"), Some([1, 2, 3, 4]));
        assert_eq!(parse_ipv4("1.2.3"), None);
        assert!(is_reserved("192.168.1.1"));
        assert!(is_reserved("10.0.0.1"));
        assert!(is_reserved("127.0.0.1"));
        assert!(is_reserved("169.254.1.1"));
        assert!(!is_reserved("8.8.8.8"));
        assert!(is_reserved("::1"));
        assert!(is_reserved("fe80::1"));
        assert!(is_reserved("fd00::1"));
        assert!(is_reserved("2001:db8::1"));
        assert!(!is_reserved("2408:8207::1"));
    }

    #[test]
    fn test_prefix_key() {
        assert_eq!(prefix_key("1.2.3.4").as_deref(), Some("1.2"));
        assert_eq!(prefix_key("2408:8207:1::A").as_deref(), Some("2408:8207"));
        assert_eq!(prefix_key("not-an-ip"), None);
    }
}
