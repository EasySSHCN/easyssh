//! `--enroll` 模式：一次性 enrollment token 换 AgentToken（方案 6.1.2）。
//!
//! 网关侧约定（P4 落地 `POST /agent/enroll`；本端首版按此契约实现）：
//! - 请求：`POST {gateway}/agent/enroll`，JSON
//!   `{"enrollment_token": "..."}`；
//! - 响应 200：`{"agent_token": "...", "resource_id": "..."}`
//!   （resource_id 回读用于写入配置，防 token 与配置错绑）；
//! - enrollment token 一次性、10 分钟有效、绑定 resource_id（网关侧职责）。
//!
//! 换取成功后立即走 [`crate::store`] 落盘（Linux 0600 / Windows DPAPI）。

use crate::config::AgentConfig;
use crate::store;

#[derive(Debug, serde::Serialize)]
pub struct EnrollRequest<'a> {
    pub enrollment_token: &'a str,
}

#[derive(Debug, serde::Deserialize)]
pub struct EnrollResponse {
    pub agent_token: String,
    pub resource_id: String,
}

#[derive(Debug)]
pub enum EnrollError {
    Http(String),
    BadResponse(String),
    ResourceMismatch { token_rid: String, config_rid: String },
    Store(store::StoreError),
}

impl std::fmt::Display for EnrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnrollError::Http(m) => write!(f, "enroll 请求失败: {m}"),
            EnrollError::BadResponse(m) => write!(f, "enroll 响应非法: {m}"),
            EnrollError::ResourceMismatch { token_rid, config_rid } => write!(
                f,
                "enrollment token 绑定资源 {token_rid} 与配置资源 {config_rid} 不一致"
            ),
            EnrollError::Store(e) => write!(f, "token 落盘失败: {e}"),
        }
    }
}

impl std::error::Error for EnrollError {}

/// 执行 enroll：HTTP 换票 → 校验 resource_id 绑定 → 落盘 → 返回落盘路径。
///
/// 一次性阻塞调用（enroll 是安装期一次性动作，不需要异步运行时）。
pub fn enroll_and_store(
    cfg: &AgentConfig,
    enrollment_token: &str,
) -> Result<std::path::PathBuf, EnrollError> {
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(cfg.accept_invalid_certs)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| EnrollError::Http(e.to_string()))?;

    let url = format!("{}/agent/enroll", cfg.http_base());
    let resp = client
        .post(&url)
        .json(&EnrollRequest { enrollment_token })
        .send()
        .map_err(|e| EnrollError::Http(format!("POST {url}: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(EnrollError::Http(format!("POST {url}: HTTP {status}: {body}")));
    }

    let parsed: EnrollResponse = resp
        .json()
        .map_err(|e| EnrollError::BadResponse(e.to_string()))?;
    if parsed.agent_token.is_empty() {
        return Err(EnrollError::BadResponse("agent_token 为空".into()));
    }
    if parsed.resource_id != cfg.resource_id {
        return Err(EnrollError::ResourceMismatch {
            token_rid: parsed.resource_id,
            config_rid: cfg.resource_id.clone(),
        });
    }

    store::save_token(&parsed.agent_token, cfg.token_path.as_deref().map(|p| p as &std::path::Path))
        .map_err(EnrollError::Store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn tmp_token_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("easyssh-agent-enroll-{name}-{}", std::process::id()))
    }

    /// 起一次性本地 HTTP 服务充当假网关 enroll 端点。
    fn spawn_fake_enroll(
        body: &'static str,
        status_line: &'static str,
    ) -> (u16, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf); // 丢弃请求头（本测试不校验 body 编码细节）
            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status_line,
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        (port, h)
    }

    fn cfg_for(port: u16, rid: &str, token_path: std::path::PathBuf) -> AgentConfig {
        AgentConfig {
            gateway_url: format!("http://127.0.0.1:{port}"),
            resource_id: rid.to_string(),
            local_backend: "127.0.0.1:22".into(),
            heartbeat_secs: 30,
            dial_timeout_secs: 10,
            max_sessions: 8,
            backoff_base_ms: 1_000,
            backoff_cap_ms: 30_000,
            accept_invalid_certs: false,
            token_path: Some(token_path),
        }
    }

    #[test]
    fn enroll_success_persists_token() {
        let body = r#"{"agent_token":"agent.jwt.value","resource_id":"u-abc"}"#;
        let (port, th) = spawn_fake_enroll(body, "200 OK");
        let path = tmp_token_path("ok");
        let cfg = cfg_for(port, "u-abc", path.clone());
        let saved = enroll_and_store(&cfg, "enrollment-token-1").expect("enroll ok");
        assert_eq!(saved, path);
        assert_eq!(
            store::load_token(Some(&path)).expect("load"),
            Some("agent.jwt.value".to_string())
        );
        let _ = store::clear_token(Some(&path));
        th.join().ok();
    }

    #[test]
    fn enroll_resource_mismatch_rejected() {
        let body = r#"{"agent_token":"agent.jwt.value","resource_id":"u-other"}"#;
        let (port, th) = spawn_fake_enroll(body, "200 OK");
        let path = tmp_token_path("mismatch");
        let cfg = cfg_for(port, "u-abc", path.clone());
        let err = enroll_and_store(&cfg, "enrollment-token-1").expect_err("mismatch");
        assert!(matches!(err, EnrollError::ResourceMismatch { .. }));
        // 拒绝时不得留下 token 落盘
        assert!(!path.is_file());
        th.join().ok();
    }

    #[test]
    fn enroll_server_error_surfaces_status() {
        let (port, th) = spawn_fake_enroll(r#"{"error":"expired"}"#, "400 Bad Request");
        let path = tmp_token_path("err");
        let cfg = cfg_for(port, "u-abc", path.clone());
        let err = enroll_and_store(&cfg, "expired-token").expect_err("400");
        match err {
            EnrollError::Http(m) => assert!(m.contains("400"), "{m}"),
            other => panic!("期望 Http 错误，实际 {other:?}"),
        }
        assert!(!path.is_file());
        th.join().ok();
    }

    #[test]
    fn enroll_bad_json_rejected() {
        let (port, th) = spawn_fake_enroll("not json", "200 OK");
        let path = tmp_token_path("badjson");
        let cfg = cfg_for(port, "u-abc", path.clone());
        assert!(matches!(
            enroll_and_store(&cfg, "t").expect_err("bad json"),
            EnrollError::BadResponse(_)
        ));
        assert!(!path.is_file());
        th.join().ok();
    }
}
