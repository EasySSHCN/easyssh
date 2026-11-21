//! v5.2.x / 配置打包导出导入(.esshbak) + WebDAV 可选云同步（纯后端）。
//!
//! # .esshbak 信封字节布局（v1）
//! ```text
//! offset  size        field
//! 0       8           magic "ESSHBAK1"（ASCII）
//! 8       4           u32 LE: comment 长度 N
//! 12      N           结构说明注释（UTF-8，可读性用，解析时可忽略）
//! 12+N    4           u32 LE: wrap 段长度 W
//! 16+N    W           wrap 字符串（UTF-8）= `wrap:v3:<salt>:<iv>:<tag>:<ct>`
//!                       —— easyssh-core crypto::wrap_dek(Argon2id m=64MiB,t=3,p=1)
//!                          包裹的 32B 随机包内 DEK；salt(16B)/nonce(12B) 随包存储
//! 16+N+W  4           u32 LE: enc 段长度 E
//! 24+N+W  E           enc 字符串（UTF-8）= `enc:v1:<iv>:<tag>:<ct>`
//!                       —— 同一 DEK 对 payload JSON 的 AES-256-GCM 密文
//! ```
//! payload（加密前的 JSON）：`{ version:1, createdAt:<unix秒>, app:"easyssh",
//!   data: { config: {sessions, settings}, knownHosts: {...} } }`
//!
//! **不打包 keys/ 目录**：私钥文件由 Windows DPAPI 绑定本机用户解密，跨机迁移必然
//! 失败且可能诱导用户在目标机上错误配置；keys 请用 key_install / 各自生成重建。
//!
//! 复用 crates/easyssh-core/src/crypto.rs 公开 API（不改动它）：
//! `wrap_dek` / `unwrap_dek`（Argon2id KDF + AES-256-GCM 包裹）、
//! `encrypt_secret` / `decrypt_secret`（AES-256-GCM 载荷加解密）、
//! `config::generate_dek`（32B 随机 DEK）。
use std::io::Read;

use serde::Deserialize;
use serde_json::{json, Value};

use easyssh_core::config::generate_dek;
use easyssh_core::crypto::{decrypt_secret, encrypt_secret, unwrap_dek, wrap_dek};

pub const MAGIC: &[u8; 8] = b"ESSHBAK1";

const ENVELOPE_COMMENT: &str = "EasySSH config package v1\n\
     layout: magic(8) | u32le commentLen | comment | u32le wrapLen | wrap:v3 string | u32le encLen | enc:v1 string\n\
     wrap:v3 = Argon2id(m=64MiB,t=3,p=1) passphrase -> KEK, AES-256-GCM wrapped random 32B package DEK (salt 16B random, in-package)\n\
     enc:v1  = AES-256-GCM(payload JSON) with that DEK (nonce 12B random)\n\
     NOTE: keys/ directory is intentionally NOT packaged (DPAPI machine-bound, not migratable).";

/* ============================ 纯函数（可测） ============================ */

/// 会话键 host:port:user（port 缺省 22；username/user 兼容取）——与 configstate 一致
pub fn session_key(rec: &Value) -> Option<String> {
    let host = rec.get("host").and_then(|x| x.as_str())?;
    if host.is_empty() {
        return None;
    }
    let port = rec.get("port").and_then(|x| x.as_u64()).unwrap_or(22);
    let user = rec
        .get("username")
        .or_else(|| rec.get("user"))
        .and_then(|x| x.as_str())
        .unwrap_or("");
    Some(format!("{}:{}:{}", host, port, user))
}

/// 组装 payload Value
pub fn build_payload(config: &Value, known_hosts: &Value) -> Value {
    json!({
        "version": 1,
        "createdAt": unix_secs(),
        "app": "easyssh",
        "data": { "config": config, "knownHosts": known_hosts },
    })
}

/// 打包：payload JSON → 随机 DEK 加密 + 口令包裹 → .esshbak 信封字节
pub fn seal_envelope(payload_json: &str, passphrase: &str) -> Result<Vec<u8>, String> {
    check_passphrase_strength(passphrase)?;
    let dek = generate_dek();
    let wrapped = wrap_dek(&dek, passphrase)?;
    let enc = encrypt_secret(payload_json, &dek)?;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    put_u32(&mut out, ENVELOPE_COMMENT.len() as u32);
    out.extend_from_slice(ENVELOPE_COMMENT.as_bytes());
    put_u32(&mut out, wrapped.len() as u32);
    out.extend_from_slice(wrapped.as_bytes());
    put_u32(&mut out, enc.len() as u32);
    out.extend_from_slice(enc.as_bytes());
    Ok(out)
}

/// 解包：校验 magic → 口令解包 DEK（错口令明确报错）→ 解密 payload JSON
pub fn open_envelope(bytes: &[u8], passphrase: &str) -> Result<String, String> {
    let mut cur = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        if cur + n > bytes.len() {
            return Err("信封格式无效(截断)".into());
        }
        let s = &bytes[cur..cur + n];
        cur += n;
        Ok(s)
    };
    if take(8)? != MAGIC {
        return Err("不是有效的 .esshbak 文件(magic 不匹配)".into());
    }
    let clen = get_u32(take(4)?)? as usize;
    let _comment = std::str::from_utf8(take(clen)?).map_err(|_| "信封注释损坏".to_string())?;
    let wlen = get_u32(take(4)?)? as usize;
    let wrapped = String::from_utf8(take(wlen)?.to_vec()).map_err(|_| "wrap 段损坏".to_string())?;
    let elen = get_u32(take(4)?)? as usize;
    let enc = String::from_utf8(take(elen)?.to_vec()).map_err(|_| "enc 段损坏".to_string())?;
    drop(take);
    let dek = unwrap_dek(&wrapped, passphrase).map_err(|e| format!("口令错误或包已损坏: {e}"))?;
    decrypt_secret(&enc, &dek)
}

/// 预检：对当前 sessions/settings 与包内 payload 计算导入报告（不写盘）
pub fn import_report(current_cfg: &Value, payload: &Value) -> Result<Value, String> {
    let data = payload
        .get("data")
        .ok_or("payload 缺少 data 字段")?;
    let incoming_sessions = data
        .pointer("/config/sessions")
        .and_then(|x| x.as_array())
        .ok_or("payload 缺少 data.config.sessions")?;
    let incoming_kh = data.get("knownHosts").cloned().unwrap_or_else(|| json!({}));
    let current_sessions = current_cfg
        .pointer("/sessions")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let current_kh = current_cfg
        .pointer("/settings/knownHosts")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let existing: std::collections::HashSet<String> = current_sessions
        .iter()
        .filter_map(session_key)
        .collect();
    let mut n_new = 0usize;
    let mut conflicts: Vec<String> = Vec::new();
    for s in incoming_sessions {
        match session_key(s) {
            Some(k) => {
                if existing.contains(&k) {
                    if conflicts.len() < 50 {
                        conflicts.push(k);
                    }
                } else {
                    n_new += 1;
                }
            }
            None => {} // 无 host 的残缺记录不计
        }
    }
    let kh_new = count_new_keys(&current_kh, &incoming_kh);
    Ok(json!({
        "sessionsTotal": incoming_sessions.len(),
        "sessionsNew": n_new,
        "sessionsUpdate": conflicts.len(),
        "sessionsConflict": conflicts,
        "knownHostsNew": kh_new,
    }))
}

/// merge 决策（保守策略）：按 host:port:user 键，不存在→追加(imported)；
/// 已存在→保留现有(skipped/conflict)，绝不覆盖。返回 (合并后 sessions, imported, skipped, conflicts≤50)
pub fn merge_sessions(
    current: &[Value],
    incoming: &[Value],
) -> (Vec<Value>, usize, usize, Vec<String>) {
    let mut merged: Vec<Value> = current.to_vec();
    let mut seen: std::collections::HashSet<String> =
        current.iter().filter_map(session_key).collect();
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut conflicts: Vec<String> = Vec::new();
    for s in incoming {
        match session_key(s) {
            Some(k) => {
                if seen.contains(&k) {
                    skipped += 1;
                    if conflicts.len() < 50 {
                        conflicts.push(k);
                    }
                } else {
                    seen.insert(k);
                    merged.push(s.clone());
                    imported += 1;
                }
            }
            None => {} // 残缺记录跳过
        }
    }
    (merged, imported, skipped, conflicts)
}

/// knownHosts 合并：以现有为准追加新键。返回 (合并后, 新增数)
pub fn merge_known_hosts(current: &Value, incoming: &Value) -> (Value, usize) {
    let mut merged = current.as_object().cloned().unwrap_or_default();
    let mut n = 0usize;
    if let Some(in_obj) = incoming.as_object() {
        for (k, v) in in_obj {
            if !merged.contains_key(k) {
                merged.insert(k.clone(), v.clone());
                n += 1;
            }
        }
    }
    (Value::Object(merged), n)
}

/// WebDAV URL 归一化拼接：基础 URL 去末尾斜杠 + 子路径去首尾斜杠 + '/' + 文件名
pub fn webdav_join(base: &str, filename: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let sub = filename.trim().trim_matches('/');
    if sub.is_empty() {
        return base.to_string();
    }
    format!("{base}/{sub}")
}

/// 口令强度：导出/推送时 ≥8 位
pub fn check_passphrase_strength(pw: &str) -> Result<(), String> {
    if pw.chars().count() < 8 {
        return Err("口令至少需要 8 位".into());
    }
    Ok(())
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn get_u32(b: &[u8]) -> Result<u32, String> {
    if b.len() != 4 {
        return Err("信封长度字段损坏".into());
    }
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn count_new_keys(current: &Value, incoming: &Value) -> usize {
    let cur = current.as_object();
    match incoming.as_object() {
        Some(in_obj) => in_obj
            .keys()
            .filter(|k| cur.map(|c| !c.contains_key(*k)).unwrap_or(true))
            .count(),
        None => 0,
    }
}

pub fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/* ============================ WebDAV ============================ */

#[derive(Debug, Clone, Deserialize)]
pub struct WebDavCfg {
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_filename")]
    pub filename: String,
}

fn default_filename() -> String {
    "easyssh-backup.esshbak".into()
}

/// 构造带 Basic auth + 超时的 ureq 请求（密码绝不进日志/错误信息）
fn dav_agent(cfg: &WebDavCfg) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

fn dav_auth(cfg: &WebDavCfg) -> Option<String> {
    if cfg.username.is_empty() && cfg.password.is_empty() {
        return None;
    }
    use base64::Engine;
    Some(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!(
            "{}:{}",
            cfg.username, cfg.password
        ))
    ))
}

fn dav_request(
    cfg: &WebDavCfg,
    method: &str,
    url: &str,
    body: Option<Vec<u8>>,
) -> Result<ureq::Response, String> {
    let agent = dav_agent(cfg);
    let mut req = agent.request(method, url);
    if let Some(auth) = dav_auth(cfg) {
        req = req.set("Authorization", &auth);
    }
    let result = match body {
        Some(b) => req.send_bytes(&b),
        None => req.call(),
    };
    match result {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(code, resp)) => {
            Err(format!("WebDAV {method} 失败: HTTP {code} {}", url_for_log(url)))
        }
        Err(ureq::Error::Transport(t)) => {
            Err(format!("WebDAV {method} 传输失败: {}", t))
        }
    }
}

/// 日志用 URL：剥掉可能内嵌在 URL 里的 userinfo 凭据
fn url_for_log(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at) = rest.find('@') {
            let host = &rest[at + 1..];
            if rest[..at].contains(':') && !host.is_empty() && !host.contains('@') {
                return format!("{}://<redacted>@{}", &url[..scheme_end], host);
            }
        }
    }
    url.to_string()
}

/// PROPFIND(207)/HEAD 探测目录可达 + 认证 OK
pub fn webdav_test(cfg: &WebDavCfg) -> Result<Value, String> {
    let dir_url = webdav_join(&cfg.url, "");
    let file_url = webdav_join(&cfg.url, &cfg.filename);
    // 先 PROPFIND 目录（Depth:0）；部分简化服务器不支持 → 退回 HEAD 文件
    match dav_request(cfg, "PROPFIND", &dir_url, None) {
        Ok(r) => {
            let code = r.status();
            if (200..300).contains(&code) {
                return Ok(json!({ "ok": true, "status": code, "probe": "PROPFIND" }));
            }
            if code == 401 || code == 403 {
                return Err(format!("WebDAV 认证失败: HTTP {code}"));
            }
        }
        Err(e) => {
            if e.contains("HTTP 401") || e.contains("HTTP 403") {
                return Err("WebDAV 认证失败: 用户名或密码错误".into());
            }
        }
    }
    let r = dav_request(cfg, "HEAD", &file_url, None)
        .map_err(|e| format!("目录探测失败({e}); HEAD 也失败"))?;
    let code = r.status();
    if code == 401 || code == 403 {
        return Err(format!("WebDAV 认证失败: HTTP {code}"));
    }
    // 404 也算可达（文件尚不存在），认证通过即可
    Ok(json!({ "ok": true, "status": code, "probe": "HEAD" }))
}

/// PUT 上传（先 MKCOL 目录可选兜底：404/409 时建父目录重试一次）
pub fn webdav_push(cfg: &WebDavCfg, data: &[u8]) -> Result<(), String> {
    let file_url = webdav_join(&cfg.url, &cfg.filename);
    match dav_request(cfg, "PUT", &file_url, Some(data.to_vec())) {
        Ok(r) => {
            let code = r.status();
            if (200..300).contains(&code) {
                return Ok(());
            }
            Err(format!("WebDAV PUT 失败: HTTP {code}"))
        }
        Err(e) => {
            let needs_mkdir = e.contains("HTTP 404") || e.contains("HTTP 409");
            if !needs_mkdir {
                return Err(e);
            }
            // 尝试创建父目录后重试一次
            let dir_url = webdav_join(&cfg.url, "");
            let _ = dav_request(cfg, "MKCOL", &dir_url, None);
            let r = dav_request(cfg, "PUT", &file_url, Some(data.to_vec()))?;
            let code = r.status();
            if (200..300).contains(&code) {
                Ok(())
            } else {
                Err(format!("WebDAV PUT 失败(已尝试 MKCOL): HTTP {code}"))
            }
        }
    }
}

/// GET 下载到内存
pub fn webdav_pull(cfg: &WebDavCfg) -> Result<Vec<u8>, String> {
    let file_url = webdav_join(&cfg.url, &cfg.filename);
    let mut resp = dav_request(cfg, "GET", &file_url, None)?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| format!("WebDAV 下载读取失败: {e}"))?;
    if buf.is_empty() {
        return Err("WebDAV 远端文件为空".into());
    }
    Ok(buf)
}

/* ============================ 命令层（供 main.rs 包装） ============================ */

pub struct CurrentConfig {
    pub sessions: Value,
    pub settings: Value,
}

/// 读当前解密后的 sessions/settings
pub fn read_current(state: &crate::configstate::ConfigState) -> Result<CurrentConfig, String> {
    let v = state.load_value()?;
    Ok(CurrentConfig {
        sessions: v.get("sessions").cloned().unwrap_or_else(|| json!([])),
        settings: v.get("settings").cloned().unwrap_or_else(|| json!({})),
    })
}

/// 导出：读 config + knownHosts → 组 payload → 加密 → 写 outPath。
/// v5.3/R-20261120-005：payload 剥离网关 token/password/OTP 字段（主 + fallbacks）。
pub fn do_export(
    state: &crate::configstate::ConfigState,
    passphrase: &str,
    out_path: &str,
) -> Result<Value, String> {
    let cur = read_current(state)?;
    let mut sessions_out = cur.sessions.clone();
    crate::gateway::strip_gateway_secrets(&mut sessions_out);
    let kh = cur.settings.get("knownHosts").cloned().unwrap_or_else(|| json!({}));
    let payload = build_payload(
        &json!({ "sessions": sessions_out, "settings": cur.settings }),
        &kh,
    );
    let bytes = seal_envelope(&payload.to_string(), passphrase)?;
    easyssh_core::config::write_file_atomic(std::path::Path::new(out_path), &bytes)?;
    let sessions = payload
        .pointer("/data/config/sessions")
        .and_then(|x| x.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Ok(json!({ "path": out_path, "sessions": sessions }))
}

/// 导入前公共步骤：读文件 → 解密 → payload
fn load_package(path: &str, passphrase: &str) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("读取文件失败: {e}"))?;
    let json = open_envelope(&bytes, passphrase)?;
    serde_json::from_str(&json).map_err(|e| format!("payload 解析失败: {e}"))
}

/// 预检（不写盘）
pub fn do_preflight(
    state: &crate::configstate::ConfigState,
    passphrase: &str,
    file_path: &str,
) -> Result<Value, String> {
    let payload = load_package(file_path, passphrase)?;
    let cur = read_current(state)?;
    let cur_val = json!({
        "sessions": cur.sessions,
        "settings": cur.settings,
    });
    let mut report = import_report(&cur_val, &payload)?;
    if let Some(o) = report.as_object_mut() {
        o.insert("createdAt".into(), json!(payload.get("createdAt").cloned().unwrap_or(json!(0))));
        o.insert("app".into(), json!(payload.get("app").cloned().unwrap_or(json!(""))));
    }
    Ok(report)
}

/// 导入前备份 config.json → config.json.bak-import-<unix秒>（单份滚动覆盖）
pub fn backup_config_file(state: &crate::configstate::ConfigState) -> Result<String, String> {
    let src = state.store.config_file();
    let dst = src.with_file_name(format!("config.json.bak-import-{}", unix_secs()));
    let _ = std::fs::remove_file(&dst); // 滚动覆盖
    std::fs::copy(&src, &dst).map_err(|e| format!("备份 config.json 失败: {e}"))?;
    Ok(dst.to_string_lossy().to_string())
}

/// 导入 merge/replace（先备份，写盘走 ConfigState::save_value 保持加密语义）
pub fn do_import(
    state: &crate::configstate::ConfigState,
    passphrase: &str,
    file_path: &str,
    mode: &str,
) -> Result<Value, String> {
    let payload = load_package(file_path, passphrase)?;
    let data = payload.get("data").ok_or("payload 缺少 data 字段")?;
    let incoming_sessions = data
        .pointer("/config/sessions")
        .and_then(|x| x.as_array())
        .ok_or("payload 缺少 data.config.sessions")?
        .clone();
    let incoming_kh = data.get("knownHosts").cloned().unwrap_or_else(|| json!({}));
    let cur = read_current(state)?;

    backup_config_file(state)?;

    let (merged_sessions, imported, skipped, conflicts) = match mode {
        "merge" => merge_sessions(
            cur.sessions.as_array().unwrap_or(&Vec::new()),
            &incoming_sessions,
        ),
        "replace" => {
            // 整包替换 sessions；残缺记录（无 host）一并替换进去由用户自行清理
            let n = incoming_sessions.len();
            (incoming_sessions.clone(), n, 0usize, Vec::new())
        }
        other => return Err(format!("未知 mode: {other}（仅支持 merge/replace）")),
    };

    let (merged_kh, _kh_new) =
        merge_known_hosts(cur.settings.get("knownHosts").unwrap_or(&json!({})), &incoming_kh);
    let mut new_settings = cur.settings.clone();
    if let Some(o) = new_settings.as_object_mut() {
        o.insert("knownHosts".into(), merged_kh);
    }

    state
        .save_value(json!(merged_sessions), new_settings)
        .map_err(|e| format!("保存导入结果失败: {e}"))?;

    Ok(json!({ "imported": imported, "skipped": skipped, "conflicts": conflicts, "mode": mode }))
}

/* ============================ 测试 ============================ */

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(host: &str, port: u64, user: &str) -> Value {
        json!({ "host": host, "port": port, "username": user })
    }

    #[test]
    fn session_key_defaults() {
        assert_eq!(session_key(&s("h", 22, "u")).as_deref(), Some("h:22:u"));
        assert_eq!(
            session_key(&json!({"host":"h","username":"u"})).as_deref(),
            Some("h:22:u")
        );
        assert_eq!(session_key(&json!({"host":"","port":1})), None);
        assert_eq!(session_key(&json!({"port":22})), None);
    }

    #[test]
    fn envelope_roundtrip_merge_replace() {
        let payload = build_payload(
            &json!({"sessions": [s("a",22,"u"), s("b",2200,"v")], "settings": {}}),
            &json!({"a:22": "fp1"}),
        );
        let bytes = seal_envelope(&payload.to_string(), "strong-pass-1").unwrap();
        assert!(bytes.starts_with(MAGIC));
        // roundtrip
        let back = open_envelope(&bytes, "strong-pass-1").unwrap();
        let v: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(v["app"], "easyssh");
        assert_eq!(v["version"], 1);
        assert_eq!(v["data"]["knownHosts"]["a:22"], "fp1");
        // 错口令拒绝
        let err = open_envelope(&bytes, "wrong-passphrase").unwrap_err();
        assert!(err.contains("口令错误"), "got: {err}");
        // magic 破坏拒绝
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(open_envelope(&bad, "strong-pass-1").is_err());
        // 截断拒绝
        assert!(open_envelope(&bytes[..bytes.len() - 3], "strong-pass-1").is_err());
    }

    #[test]
    fn short_passphrase_rejected() {
        let err = seal_envelope("{}", "short").unwrap_err();
        assert!(err.contains("8 位"));
        assert!(seal_envelope("{}", "12345678").is_ok());
    }

    #[test]
    fn merge_conflict_decisions() {
        let cur = vec![s("a", 22, "u"), s("b", 22, "u")];
        let incoming = vec![
            s("a", 22, "u"),   // 冲突：保留现有
            s("c", 22, "w"),   // 新增
            s("b", 22, "u"),   // 冲突
            json!({"host": "d"}), // 无 user，键 d:22:"" → 新增
            json!({"port": 1}),   // 残缺 → 跳过
        ];
        let (merged, imported, skipped, conflicts) = merge_sessions(&cur, &incoming);
        assert_eq!(imported, 2);
        assert_eq!(skipped, 2);
        assert_eq!(conflicts, vec!["a:22:u", "b:22:u"]);
        assert_eq!(merged.len(), 4);
        // 现有记录未被覆盖
        assert_eq!(merged[0]["username"], "u");
    }

    #[test]
    fn conflict_list_capped_at_50() {
        let cur: Vec<Value> = (0..60).map(|i| s(&format!("h{i}"), 22, "u")).collect();
        let incoming = cur.clone();
        let (_, imported, skipped, conflicts) = merge_sessions(&cur, &incoming);
        assert_eq!(imported, 0);
        assert_eq!(skipped, 60);
        assert_eq!(conflicts.len(), 50);
    }

    #[test]
    fn import_report_counts() {
        let current = json!({
            "sessions": [s("a",22,"u")],
            "settings": {"knownHosts": {"a:22": "fp-old"}},
        });
        let payload = build_payload(
            &json!({"sessions": [s("a",22,"u"), s("b",22,"u")], "settings": {}}),
            &json!({"a:22": "fp-old", "b:22": "fp-new"}),
        );
        let r = import_report(&current, &payload).unwrap();
        assert_eq!(r["sessionsTotal"], 2);
        assert_eq!(r["sessionsNew"], 1);
        assert_eq!(r["sessionsUpdate"], 1);
        assert_eq!(r["sessionsConflict"][0], "a:22:u");
        assert_eq!(r["knownHostsNew"], 1);
    }

    #[test]
    fn known_hosts_merge_keeps_existing() {
        let (merged, n) = merge_known_hosts(
            &json!({"a:22": "old"}),
            &json!({"a:22": "should-not-overwrite", "b:22": "new"}),
        );
        assert_eq!(n, 1);
        assert_eq!(merged["a:22"], "old"); // 现有指纹不被覆盖
        assert_eq!(merged["b:22"], "new");
    }

    #[test]
    fn webdav_url_normalization() {
        assert_eq!(
            webdav_join("https://dav.example.com/dav/", "easyssh-backup.esshbak"),
            "https://dav.example.com/dav/easyssh-backup.esshbak"
        );
        assert_eq!(
            webdav_join("https://dav.example.com", "sub/easyssh-backup.esshbak"),
            "https://dav.example.com/sub/easyssh-backup.esshbak"
        );
        assert_eq!(webdav_join(" https://x.com/dir ", " /f.bin "), "https://x.com/dir/f.bin");
        assert_eq!(webdav_join("https://x.com/", "/"), "https://x.com");
    }

    #[test]
    fn url_log_redacts_userinfo() {
        assert_eq!(
            url_for_log("https://user:secret@dav.example.com/f"),
            "https://<redacted>@dav.example.com/f"
        );
        assert_eq!(url_for_log("https://dav.example.com/f"), "https://dav.example.com/f");
    }

    #[test]
    fn payload_structure() {
        let p = build_payload(&json!({"sessions": []}), &json!({}));
        assert_eq!(p["version"], 1);
        assert_eq!(p["app"], "easyssh");
        assert!(p["createdAt"].as_u64().unwrap() > 0);
        assert!(p["data"]["config"].is_object());
        assert!(p["data"]["knownHosts"].is_object());
    }
}
