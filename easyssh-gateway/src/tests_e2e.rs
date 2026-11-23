//! 端到端集成测试：
//! 1. **占位登录安全回归四用例**（R-20261120-006/007/008）：
//!    - dev_mode=false 占位 POST 必须 401 + auth_placeholder_denied 审计；
//!    - dev_mode=true 白名单外拒绝并有审计事件；
//!    - dev_mode=true 白名单内占位签发（角色按矩阵受限）；
//!    - 真实 OIDC code 正常换票（MockIdp 注入，不联网）。
//! 2. 隧道全链路：WS Binary 帧 ↔ TCP echo 后端字节流一致；
//! 3. wss 回环：自签证书 TLS 全链路 echo；
//! 4. OTP 二次认证：缺码 401 otp_required，正确码换票。

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::auth::AuthState;
    use crate::resources::ResourceRegistry;
    use crate::{tunnel, AppState};

    /// 启动 echo 后端（读到什么回什么），返回监听地址。
    async fn spawn_echo_backend() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let mut buf = [0u8; 4096];
                        loop {
                            match sock.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    if sock.write_all(&buf[..n]).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    });
                }
            }
        });
        addr
    }

    /// e2e 宽松策略：`*` × `*` × connect/list 全 allow
    /// （默认矩阵 deny by default 会拒绝测试用户的 connect）。
    fn permissive_policy() -> crate::policy::PolicyHandle {
        let pc = crate::policy::PolicyConfig {
            rules: vec![
                crate::policy::PolicyRuleConfig {
                    resource: "*".into(),
                    role: "*".into(),
                    action: "connect".into(),
                    decision: "allow".into(),
                },
                crate::policy::PolicyRuleConfig {
                    resource: "*".into(),
                    role: "*".into(),
                    action: "list".into(),
                    decision: "allow".into(),
                },
            ],
        };
        crate::policy::PolicyHandle::from_matrix(
            crate::policy::PolicyMatrix::from_config(&pc).expect("compile permissive matrix"),
        )
    }

    /// 启动 axum gateway 在 127.0.0.1 临时端口，返回 (监听地址, AppState)。
    async fn spawn_gateway(state: Arc<AppState>) -> SocketAddr {
        let app = axum::Router::new()
            .merge(crate::auth::router(Arc::clone(&state)))
            .merge(tunnel::router(Arc::clone(&state)))
            .merge(crate::agent::router())
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("gateway serve");
        });
        addr
    }

    fn base_state(dev_mode: bool, cidrs: &[&str]) -> Arc<AppState> {
        let cidrs: Vec<String> = cidrs.iter().map(|s| s.to_string()).collect();
        let dev_cidrs = crate::cidr::parse_cidrs(&cidrs).expect("valid cidrs");
        Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::default(),
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: Default::default(),
            oidc: None,
            dev_mode,
            dev_cidrs: Arc::new(dev_cidrs),
        agent_hub: crate::agent::AgentHub::new(),
        agent_cfg: Default::default(),
        agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        })
    }

    async fn post_login(
        addr: SocketAddr,
        body: &str,
        local_ip: Option<&str>,
    ) -> (reqwest::StatusCode, String) {
        let mut builder = reqwest::Client::builder();
        if let Some(ip) = local_ip {
            builder = builder.local_address(Some(ip.parse::<std::net::IpAddr>().unwrap()));
        }
        let client = builder.build().unwrap();
        let resp = client
            .post(format!("http://{addr}/api/auth/login"))
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("login request");
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        (status, text)
    }

    // -----------------------------------------------------------------
    // 占位登录安全回归四用例
    // -----------------------------------------------------------------

    /// 用例 1：dev_mode=false（生产口径默认），占位 POST 必须 401，
    /// 且审计含 auth_placeholder_denied。
    #[tokio::test]
    async fn placeholder_login_dev_mode_false_denied_401_with_audit() {
        let state = base_state(false, &["127.0.0.1"]);
        let addr = spawn_gateway(Arc::clone(&state)).await;
        // 即便来源是回环（通常在白名单内），dev_mode=false 一票否决
        let (status, body) = post_login(addr, r#"{"oidc_code":"intruder:admin"}"#, None).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "dev_mode=false 必须 401");
        assert!(state
            .audit
            .snapshot()
            .iter()
            .any(|e| e.event == "auth_placeholder_denied" && e.source_ip == "127.0.0.1"),
        "必须记录 auth_placeholder_denied 审计事件");
        // 401 响应不含任何 token
        assert!(!body.contains("\"token\""), "401 响应不得泄露 token: {body}");
    }

    /// 用例 2：dev_mode=true 但来源 IP 在白名单外 —— 拒绝 + 审计事件。
    /// 用真实 socket 从 127.0.0.9 发起（Windows 回环支持整个 127/8），
    /// 白名单只允许 127.0.0.1/32。
    #[tokio::test]
    async fn placeholder_login_dev_mode_true_outside_allowlist_denied() {
        let state = base_state(true, &["127.0.0.1/32"]);
        let addr = spawn_gateway(Arc::clone(&state)).await;
        let (status, _body) = post_login(addr, r#"{"oidc_code":"intruder:admin"}"#, Some("127.0.0.9")).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED, "白名单外必须 401");
        assert!(state
            .audit
            .snapshot()
            .iter()
            .any(|e| e.event == "auth_placeholder_denied" && e.source_ip == "127.0.0.9"),
        "白名单外拒绝必须有审计事件");
    }

    /// 用例 3：dev_mode=true 且白名单内（127.0.0.1）—— 占位签发成功；
    /// 角色受限：token 角色按占位串解析（user），策略矩阵 deny by default
    /// 下不可 connect 未授权动作；admin 角色占位在矩阵层面才全通。
    #[tokio::test]
    async fn placeholder_login_dev_mode_true_inside_allowlist_issues_limited_token() {
        // 本用例用默认策略矩阵（deny by default、admin 全通）验证角色受限
        let cidrs = crate::cidr::parse_cidrs(&["127.0.0.0/8".to_string()]).unwrap();
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::default(),
            admin_key: None,
            policy: crate::policy::PolicyHandle::default(),
            totp_secrets: Default::default(),
            oidc: None,
            dev_mode: true,
            dev_cidrs: Arc::new(cidrs),
        agent_hub: crate::agent::AgentHub::new(),
        agent_cfg: Default::default(),
        agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        });
        let addr = spawn_gateway(Arc::clone(&state)).await;
        let (status, body) = post_login(addr, r#"{"oidc_code":"devuser:user"}"#, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "白名单内占位应放行: {body}");
        let json: serde_json::Value = serde_json::from_str(&body).expect("json");
        let token = json["token"].as_str().expect("token");
        assert_eq!(json["expires_in"], serde_json::json!(600), "token_ttl_seconds=600");
        let claims = state.auth.validate_token(token).expect("issued token validates");
        assert_eq!(claims.sub, "devuser");
        assert_eq!(claims.roles, vec!["user".to_string()], "占位角色按串解析");
        // 角色受限：user 角色在默认矩阵下 connect 被拒（策略层受限验证）
        assert_eq!(
            state.policy.check("ssh-jump01", crate::policy::canonical_role(&claims.roles), crate::policy::Action::Connect),
            crate::policy::Decision::Deny,
            "非 admin 角色默认矩阵必须 deny"
        );
        assert!(state
            .audit
            .snapshot()
            .iter()
            .any(|e| e.event == "auth_login_dev_placeholder" && e.user == "devuser"),
        "放行的占位登录也应留审计痕迹");
    }

    /// 用例 4：真实 OIDC code 不走占位闸门 —— 即使 dev_mode=false 也能
    /// 正常换票（MockIdp 内联 discovery/token/JWKS，ES256 密钥运行时生成）。
    #[tokio::test]
    async fn real_oidc_code_exchange_unaffected_by_placeholder_gate() {
        use crate::auth::{OidcState, TokenFetcher};

        struct MockIdp {
            discovery: String,
            jwks: String,
            id_token: std::sync::Mutex<String>,
        }
        fn b64url(bytes: &[u8]) -> String {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        }
        // 运行时生成 ES256 测试密钥并签 id_token
        let kp = rcgen::KeyPair::generate().expect("generate P-256 key");
        let spki = kp.public_key_der();
        let point = &spki[26..];
        let jwks = format!(
            r#"{{"keys":[{{"kty":"EC","crv":"P-256","kid":"test-key","use":"sig","x":"{}","y":"{}"}}]}}"#,
            b64url(&point[1..33]),
            b64url(&point[33..65])
        );
        const ISSUER: &str = "https://sso.test/realms/m";
        const CLIENT_ID: &str = "easyssh-gateway";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = serde_json::json!({
            "iss": ISSUER, "aud": CLIENT_ID, "sub": "oidc-user",
            "exp": now + 300, "iat": now, "roles": ["user"],
        });
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some("test-key".to_string());
        let id_token = jsonwebtoken::encode(
            &header,
            &claims,
            &jsonwebtoken::EncodingKey::from_ec_pem(kp.serialize_pem().as_bytes()).unwrap(),
        )
        .expect("sign id_token");

        impl TokenFetcher for MockIdp {
            fn exchange_code<'a>(
                &'a self,
                _token_endpoint: &'a str,
                _form: &'a [(String, String)],
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + 'a>> {
                let id_token = self.id_token.lock().unwrap().clone();
                Box::pin(async move {
                    Ok(serde_json::json!({
                        "access_token": "at-123",
                        "token_type": "Bearer",
                        "id_token": id_token,
                    }))
                })
            }
            fn fetch_text<'a>(
                &'a self,
                url: &'a str,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>> {
                let body = if url.ends_with("/jwks") {
                    self.jwks.clone()
                } else {
                    self.discovery.clone()
                };
                Box::pin(async move { Ok(body) })
            }
        }

        let oidc = OidcState::new(
            crate::config::OidcConfig {
                issuer: Some(ISSUER.to_string()),
                discovery_url: None,
                client_id: Some(CLIENT_ID.to_string()),
                client_secret: Some("topsecret".to_string()),
                redirect_uri: Some("https://gw.test/oidc/callback".to_string()),
                scopes: None,
                roles_claim: None,
            },
            Arc::new(MockIdp {
                discovery: format!(
                    r#"{{"issuer":"{ISSUER}","authorization_endpoint":"https://sso.test/auth","token_endpoint":"https://sso.test/token","jwks_uri":"https://sso.test/jwks"}}"#
                ),
                jwks,
                id_token: std::sync::Mutex::new(id_token),
            }),
        );

        // dev_mode=false：占位闸门全关，但真实 OIDC code 主路径必须可用
        let cidrs = crate::cidr::parse_cidrs(&[]).unwrap();
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::default(),
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: Default::default(),
            oidc: Some(Arc::new(oidc)),
            dev_mode: false,
            dev_cidrs: Arc::new(cidrs),
        agent_hub: crate::agent::AgentHub::new(),
        agent_cfg: Default::default(),
        agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        });
        let addr = spawn_gateway(Arc::clone(&state)).await;
        let (status, body) =
            post_login(addr, r#"{"oidc_code":"REAL-AUTHORIZATION-CODE-123"}"#, None).await;
        assert_eq!(status, reqwest::StatusCode::OK, "真实 OIDC code 应换票成功: {body}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        let token = json["token"].as_str().unwrap();
        let token_claims = state.auth.validate_token(token).unwrap();
        assert_eq!(token_claims.sub, "oidc-user");
        assert_eq!(token_claims.roles, vec!["user".to_string()]);
        assert!(state.audit.snapshot().iter().any(|e| e.event == "auth_login" && e.user == "oidc-user"));
    }

    /// 反向验证：真实 code 请求在未配置 [oidc] 时返回 400（占位已禁用、
    /// 真路径不可用 = 明确失败，不静默放行）。
    #[tokio::test]
    async fn real_oidc_code_without_config_fails_clearly() {
        let state = base_state(false, &[]);
        let addr = spawn_gateway(Arc::clone(&state)).await;
        let (status, body) = post_login(addr, r#"{"oidc_code":"SOME-CODE"}"#, None).await;
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert!(body.contains("oidc"), "应明确提示未配置: {body}");
    }

    // -----------------------------------------------------------------
    // OTP 二次认证
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn login_otp_required_then_success_with_code() {
        let secret = "JBSWY3DPEHPK3PXP";
        let cidrs = crate::cidr::parse_cidrs(&["127.0.0.0/8".to_string()]).unwrap();
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(64),
            resources: ResourceRegistry::default(),
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: {
                let mut m = std::collections::BTreeMap::new();
                m.insert("otpuser".to_string(), secret.to_string());
                m
            },
            oidc: None,
            dev_mode: true,
            dev_cidrs: Arc::new(cidrs),
        agent_hub: crate::agent::AgentHub::new(),
        agent_cfg: Default::default(),
        agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        });
        let addr = spawn_gateway(Arc::clone(&state)).await;

        // 1) 缺 OTP → 401 otp_required
        let (status, body) = post_login(addr, r#"{"oidc_code":"otpuser:user"}"#, None).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
        assert!(body.contains("otp_required"));

        // 2) 错误码 → 401
        let (status, _) = post_login(addr, r#"{"oidc_code":"otpuser:user","mfa_code":"000000"}"#, None).await;
        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);

        // 3) 正确 TOTP 码 → 200 + token
        let code = crate::auth::totp_code_at(
            secret,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();
        let (status, body) = post_login(
            addr,
            &format!(r#"{{"oidc_code":"otpuser:user","mfa_code":"{code}"}}"#),
            None,
        )
        .await;
        assert_eq!(status, reqwest::StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["token"].as_str().is_some());
    }

    // -----------------------------------------------------------------
    // 隧道全链路：WS echo（明文 ws）
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn tunnel_echo_end_to_end_via_ws() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;

        let backend = spawn_echo_backend().await;
        let state = base_state(false, &[]);
        state
            .resources
            .register("echo", backend.to_string());
        let addr = spawn_gateway(Arc::clone(&state)).await;
        let token = state.auth.issue_token("alice", &["user"]).expect("issue token");

        let mut req = format!("ws://{addr}/tunnel/echo").into_client_request().unwrap();
        req.headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let (mut ws, _resp) = tokio_tungstenite::connect_async(req).await.expect("ws connect");

        // 覆盖多帧与任意字节（含 0x00/0xFF）
        let pattern: Vec<u8> = (0..48 * 1024u32).map(|i| (i % 251) as u8).collect();
        ws.send(Message::Binary(pattern.clone())).await.unwrap();
        let mut got = Vec::new();
        while got.len() < pattern.len() {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
                .await
                .expect("echo within 10s")
                .expect("stream item")
                .expect("ws frame");
            if let Message::Binary(d) = msg {
                got.extend_from_slice(&d);
            }
        }
        assert_eq!(got, pattern, "经网关 WS 隧道回读的字节流必须与写入一致");
    }

    /// P2（R-20261122-004）全链路：假 Agent（真 WS 连 /agent/connect）
    /// + agent 型资源 + WS 客户端隧道 → echo 回读，字节级一致。
    #[tokio::test]
    async fn tunnel_agent_mode_echo_end_to_end_bytes_identical() {
        use crate::agent::{Envelope, Op};
        use crate::resources::ResourceConfig;
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;

        // 1) 目标机本地 echo 后端（Agent 在"目标机内"连它）
        let backend = spawn_echo_backend().await;

        // 2) agent 型资源 + 启用 agent 通道
        let registry_cfg = ResourceConfig {
            id: "agent-echo".into(),
            name: "内网 SSH 回环".into(),
            host: "-".into(),
            port: 0,
            allowed_roles: None,
            mode: Some("agent".into()),
            agent_local_backend: Some(backend.to_string()),
        };
        let state = Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(256),
            resources: ResourceRegistry::from_configs(&[registry_cfg]),
            admin_key: None,
            policy: permissive_policy(),
            totp_secrets: Default::default(),
            oidc: None,
            dev_mode: false,
            dev_cidrs: Arc::new(crate::cidr::loopback_defaults()),
            agent_hub: crate::agent::AgentHub::new(),
            agent_cfg: crate::config::AgentConfig {
                enabled: true,
                ..Default::default()
            },
            agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        });
        let addr = spawn_gateway(Arc::clone(&state)).await;

        // 3) 假 Agent：WS 连 /agent/connect（Bearer AgentToken），
        //    DIAL→OPEN_OK、DATA→原样回显、CLOSE→忽略。
        let agent_token = state
            .auth
            .issue_agent_token("agent-e2e", "agent-echo", 3600)
            .expect("issue agent token");
        let mut req = format!("ws://{addr}/agent/connect?resource_id=agent-echo")
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("Authorization", format!("Bearer {agent_token}").parse().unwrap());
        let (mut agent_ws, _resp) =
            tokio_tungstenite::connect_async(req).await.expect("agent ws connect");

        // 等网关注册表登记
        for _ in 0..50 {
            if state.agent_hub.is_online("agent-echo") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(state.agent_hub.is_online("agent-echo"), "假 Agent 必须在线");

        // 假 Agent 循环（独立任务）
        let agent_task = tokio::spawn(async move {
            let mut opened = std::collections::HashSet::new();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(Ok(msg)) = agent_ws.next().await {
                match msg {
                    Message::Binary(data) => {
                        buf.extend_from_slice(&data);
                        loop {
                            let Ok(Some((env, consumed))) = crate::agent::cut_envelope(&buf) else {
                                break;
                            };
                            buf.drain(..consumed);
                            match env.op {
                                Op::Dial => {
                                    opened.insert(env.session);
                                    agent_ws
                                        .send(Message::Binary(
                                            Envelope::new(Op::OpenOk, env.session, Vec::new())
                                                .to_bytes(),
                                        ))
                                        .await
                                        .ok();
                                }
                                Op::Data if opened.contains(&env.session) => {
                                    agent_ws
                                        .send(Message::Binary(
                                            Envelope::new(Op::Data, env.session, env.body)
                                                .to_bytes(),
                                        ))
                                        .await
                                        .ok();
                                }
                                Op::Close => {
                                    opened.remove(&env.session);
                                }
                                _ => {}
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });

        // 4) 客户端：WS 隧道连 agent 型资源 → 写入 → 回读字节级一致
        let token = state.auth.issue_token("alice", &["user"]).expect("issue token");
        let mut req = format!("ws://{addr}/tunnel/agent-echo").into_client_request().unwrap();
        req.headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let (mut ws, _resp) = tokio_tungstenite::connect_async(req).await.expect("client ws connect");

        let pattern: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 249) as u8).collect();
        ws.send(Message::Binary(pattern.clone())).await.unwrap();
        let mut got = Vec::new();
        while got.len() < pattern.len() {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
                .await
                .expect("echo within 10s")
                .expect("stream item")
                .expect("ws frame");
            if let Message::Binary(d) = msg {
                got.extend_from_slice(&d);
            }
        }
        assert_eq!(
            got, pattern,
            "经 Agent 反向隧道的回环字节流必须与写入逐字节一致"
        );
        agent_task.abort();
    }

    /// 无效 token：auth_guard 401 → WS 升级失败。
    #[tokio::test]
    async fn tunnel_rejects_bad_token() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let backend = spawn_echo_backend().await;
        let state = base_state(false, &[]);
        state.resources.register("secure", backend.to_string());
        let addr = spawn_gateway(Arc::clone(&state)).await;

        let mut req = format!("ws://{addr}/tunnel/secure").into_client_request().unwrap();
        req.headers_mut()
            .insert("Authorization", "Bearer invalid-jwt".parse().unwrap());
        let result = tokio_tungstenite::connect_async(req).await;
        assert!(result.is_err(), "无效 token 的 WS 升级必须被拒绝");
    }

    // -----------------------------------------------------------------
    // wss 回环：自签证书 TLS 全链路
    // -----------------------------------------------------------------

    fn write_self_signed_cert() -> (std::path::PathBuf, std::path::PathBuf) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("generate self-signed cert");
        let dir = std::env::temp_dir().join(format!(
            "easyssh-e2e-tls-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cert_path = dir.join("server.crt");
        let key_path = dir.join("server.key");
        std::fs::write(&cert_path, ck.cert.pem()).unwrap();
        std::fs::write(&key_path, ck.key_pair.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    /// 测试用「不校验证书」的 rustls 客户端配置（仅测试自签 wss 链路）。
    fn insecure_tls_connector() -> tokio_tungstenite::Connector {
        use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
        use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
        use rustls::{DigitallySignedStruct, SignatureScheme};

        #[derive(Debug)]
        struct NoVerify;
        impl ServerCertVerifier for NoVerify {
            fn verify_server_cert(
                &self,
                _end_entity: &CertificateDer<'_>,
                _intermediates: &[CertificateDer<'_>],
                _server_name: &ServerName<'_>,
                _ocsp_response: &[u8],
                _now: UnixTime,
            ) -> Result<ServerCertVerified, rustls::Error> {
                Ok(ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _message: &[u8],
                _cert: &CertificateDer<'_>,
                _dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &CertificateDer<'_>,
                _dss: &DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
                vec![
                    SignatureScheme::RSA_PKCS1_SHA256,
                    SignatureScheme::ECDSA_NISTP256_SHA256,
                    SignatureScheme::ED25519,
                    SignatureScheme::RSA_PSS_SHA256,
                ]
            }
        }

        let _ = rustls::crypto::ring::default_provider().install_default();
        let cfg = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        tokio_tungstenite::Connector::Rustls(Arc::new(cfg))
    }

    #[tokio::test]
    async fn wss_tunnel_echo_end_to_end_with_self_signed_cert() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::Message;

        let backend = spawn_echo_backend().await;
        let state = base_state(false, &[]);
        state.resources.register("echo", backend.to_string());
        let (cert, key) = write_self_signed_cert();

        let app = axum::Router::new()
            .merge(tunnel::router(Arc::clone(&state)))
            .with_state(Arc::clone(&state));
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
            .await
            .expect("load tls config");
        tokio::spawn(async move {
            axum_server::bind_rustls(addr, tls_cfg)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .expect("gateway https serve");
        });
        // 等待 TLS 端口就绪
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let token = state.auth.issue_token("alice", &["user"]).unwrap();
        let mut req = format!("wss://localhost:{}/tunnel/echo", addr.port())
            .into_client_request()
            .unwrap();
        req.headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse().unwrap());
        let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
            req,
            None,
            false,
            Some(insecure_tls_connector()),
        )
        .await
        .expect("wss connect (self-signed, test verifier)");

        let pattern: Vec<u8> = (0..48 * 1024u32).map(|i| (i % 251) as u8).collect();
        ws.send(Message::Binary(pattern.clone())).await.unwrap();
        let mut got = Vec::new();
        while got.len() < pattern.len() {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
                .await
                .expect("wss echo within 10s")
                .expect("stream item")
                .expect("ws frame");
            if let Message::Binary(d) = msg {
                got.extend_from_slice(&d);
            }
        }
        assert_eq!(got, pattern, "wss 隧道回读字节流必须与写入一致");
    }

    /// GET /api/resources 需认证且只含 id/name。
    #[tokio::test]
    async fn resources_endpoint_requires_bearer_and_hides_backend() {
        let backend = spawn_echo_backend().await;
        let state = base_state(false, &[]);
        state
            .resources
            .register_full("ssh-jump01", "Linux 跳板机", backend.to_string(), None);
        let addr = spawn_gateway(Arc::clone(&state)).await;
        let client = reqwest::Client::new();

        // 无 token → 401
        let resp = client
            .get(format!("http://{addr}/api/resources"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

        // 带 token → 200，无后端地址泄露
        let token = state.auth.issue_token("alice", &["user"]).unwrap();
        let resp = client
            .get(format!("http://{addr}/api/resources"))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let text = resp.text().await.unwrap();
        assert!(text.contains("ssh-jump01"));
        assert!(!text.contains(&backend.to_string()), "后端地址不得泄露: {text}");
    }

    // -----------------------------------------------------------------
    // R-20261122-004 P4：/admin/resource 动态资源 + /agent/enroll 全链路
    // -----------------------------------------------------------------

    /// P4 专用网关（含 /admin/resource + /agent/enroll + agent 通道启用）。
    async fn spawn_p4_gateway(state: Arc<AppState>) -> SocketAddr {
        let app = axum::Router::new()
            .merge(crate::dynres::router(Arc::clone(&state)))
            .merge(crate::agent::router())
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .expect("p4 gateway serve");
        });
        addr
    }

    fn p4_state(admin_key: &str) -> Arc<AppState> {
        Arc::new(AppState {
            auth: AuthState::new("e2e-test-secret", crate::auth::DEFAULT_TOKEN_TTL),
            audit: crate::audit::AuditLog::new(256),
            resources: ResourceRegistry::default(),
            admin_key: Some(admin_key.to_string().into()),
            policy: permissive_policy(),
            totp_secrets: Default::default(),
            oidc: None,
            dev_mode: false,
            dev_cidrs: Arc::new(crate::cidr::loopback_defaults()),
            agent_hub: crate::agent::AgentHub::new(),
            agent_cfg: crate::config::AgentConfig {
                enabled: true,
                max_resources_per_user: Some(2),
                ..Default::default()
            },
            agent_sessions: crate::agent::SessionSeq::new(),
            dyn_res: crate::dynres::DynRegistry::ephemeral(),
            enroll: crate::dynres::EnrollStore::new(),
        })
    }

    async fn p4_post(
        addr: SocketAddr,
        path: &str,
        admin_key: Option<&str>,
        body: &str,
    ) -> (reqwest::StatusCode, serde_json::Value) {
        let client = reqwest::Client::new();
        let mut req = client
            .post(format!("http://{addr}{path}"))
            .header("content-type", "application/json")
            .body(body.to_string());
        if let Some(k) = admin_key {
            req = req.header("X-Admin-Key", k);
        }
        let resp = req.send().await.expect("p4 post");
        let status = resp.status();
        let json: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// P4 主链路：agent 登记 → enrollment token 一次性换 AgentToken →
    /// status/update/delete + owner 绑定 + 配额 + SSRF 拒绝。
    #[tokio::test]
    async fn p4_admin_resource_and_agent_enroll_full_flow() {
        let key = "p4-admin-key";
        let state = p4_state(key);
        let addr = spawn_p4_gateway(Arc::clone(&state)).await;

        // 0) 无/错 admin key → 401
        let (st, _) = p4_post(addr, "/admin/resource", None, r#"{"action":"status"}"#).await;
        assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED);
        let (st, _) = p4_post(addr, "/admin/resource", Some("wrong"), r#"{"action":"status"}"#).await;
        assert_eq!(st, reqwest::StatusCode::UNAUTHORIZED);

        // 0b) SSRF：direct 登记回环/私网 → 400 probe_failed（拒绝私网/环回）
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"direct","owner":"u@x.cn","host":"127.0.0.1","port":22}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(j["code"], "probe_failed");
        assert!(j["message"].as_str().unwrap().contains("公网"), "SSRF 拒绝文案: {j}");
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"direct","owner":"u@x.cn","host":"192.168.1.5"}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST);
        assert!(j["message"].as_str().unwrap().contains("公网"));
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"direct","owner":"u@x.cn","host":"ssh.example.com"}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST, "DNS 主机名应被拒: {j}");

        // 1) agent 登记 → enrollment token + u- 前缀动态 ID，状态 pending
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"agent","owner":"User@X.cn","label":"内网 SSH"}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK, "agent register: {j}");
        let rid = j["resource"]["id"].as_str().expect("resource id").to_string();
        assert!(rid.starts_with("u-"), "动态 ID 必须 u- 前缀: {rid}");
        assert_eq!(j["resource"]["status"], "pending");
        assert_eq!(j["resource"]["mode"], "agent");
        let enroll_token = j["enrollment_token"].as_str().expect("enroll token").to_string();
        assert_eq!(j["enrollment_expires_in"], 600);
        assert!(state.resources.agent_backend_for(&rid, &[]).is_some(), "动态资源已并入隧道解析");

        // 2) /agent/enroll：一次性换 AgentToken（rid 绑定 + roles=["agent"]）
        let (st, j) = p4_post(
            addr,
            "/agent/enroll",
            None,
            &format!(r#"{{"enrollment_token":"{enroll_token}"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK, "enroll: {j}");
        let agent_token = j["agent_token"].as_str().expect("agent token").to_string();
        assert_eq!(j["resource_id"], rid.as_str());
        let claims = state.auth.validate_agent_token(&agent_token).expect("valid agent token");
        assert_eq!(claims.roles, vec!["agent".to_string()]);
        assert_eq!(claims.rid.as_deref(), Some(rid.as_str()));
        // 用户 JWT 与 AgentToken 互斥（issuer 隔离）
        assert!(state.auth.validate_token(&agent_token).is_err());
        // 同 token 二次消费 → 400（一次性）
        let (st, _) = p4_post(
            addr,
            "/agent/enroll",
            None,
            &format!(r#"{{"enrollment_token":"{enroll_token}"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST, "enrollment token 必须一次性");
        // 随机 token → 400
        let (st, _) = p4_post(addr, "/agent/enroll", None, r#"{"enrollment_token":"deadbeef"}"#).await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST);

        // 3) status：单条 + 全量
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            &format!(r#"{{"action":"status","id":"{rid}"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK);
        assert_eq!(j["resource"]["id"], rid.as_str());
        assert_eq!(j["resource"]["owner"], "user@x.cn", "owner 归一化为小写");
        let (st, j) = p4_post(addr, "/admin/resource", Some(key), r#"{"action":"status"}"#).await;
        assert_eq!(st, reqwest::StatusCode::OK);
        assert_eq!(j["resources"].as_array().expect("list").len(), 1);

        // 4) update：label 可改；mode 不可改；owner 不一致 403；direct backend 改私网 400
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            &format!(r#"{{"action":"update","id":"{rid}","owner":"user@x.cn","label":"改名后"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK, "update label: {j}");
        assert_eq!(j["resource"]["label"], "改名后");
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            &format!(r#"{{"action":"update","id":"{rid}","owner":"user@x.cn","mode":"direct"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST, "mode 不可修改: {j}");
        let (st, _) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            &format!(r#"{{"action":"update","id":"{rid}","owner":"other@x.cn","label":"x"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::FORBIDDEN, "owner 不一致必须 403");
        let (st, _) = p4_post(addr, "/admin/resource", Some(key), r#"{"action":"update","id":"u-none","owner":"user@x.cn"}"#).await;
        assert_eq!(st, reqwest::StatusCode::NOT_FOUND);

        // 5) 配额：该 owner 已登记 1 台，上限 2 → 第 2 台成功、第 3 台 403
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"agent","owner":"user@x.cn"}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK, "第 2 台应成功: {j}");
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"agent","owner":"user@x.cn"}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::FORBIDDEN, "超出配额必须 403: {j}");
        assert_eq!(j["code"], "quota_exceeded");
        // 其他 owner 不受影响
        let (st, _) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"agent","owner":"z@x.cn"}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK);

        // 6) delete：owner 一致 → 摘除注册表 + 解析层；status 回 404
        let (st, _) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            &format!(r#"{{"action":"delete","id":"{rid}","owner":"user@x.cn"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::OK);
        assert_eq!(
            state.resources.resolve_backend(&rid, &[]),
            crate::resources::BackendResolution::NotFound,
            "摘除后隧道解析回落 NotFound"
        );
        let (st, _) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            &format!(r#"{{"action":"status","id":"{rid}"}}"#),
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::NOT_FOUND);
        // 审计留痕齐备
        let events: Vec<String> = state.audit.snapshot().iter().map(|e| e.event.clone()).collect();
        for want in [
            "admin_resource_register",
            "agent_enroll",
            "admin_resource_update",
            "admin_resource_delete",
            "agent_enroll_rejected",
        ] {
            assert!(events.iter().any(|e| e == want), "缺少审计事件 {want}: {events:?}");
        }
    }

    /// P4：direct 登记探测链（本地 fixture：loopback 由 SSRF 白名单拒绝，
    /// banner 语义由 dynres 单测覆盖；此处验证探测失败不落库）。
    #[tokio::test]
    async fn p4_direct_register_probe_failure_leaves_no_entry() {
        let key = "p4-admin-key";
        let state = p4_state(key);
        let addr = spawn_p4_gateway(Arc::clone(&state)).await;
        // 公网不可达地址（TEST-NET-2，沙箱无路由）→ 3s 试连失败 → probe_failed，不落库
        let (st, j) = p4_post(
            addr,
            "/admin/resource",
            Some(key),
            r#"{"action":"register","mode":"direct","owner":"d@x.cn","host":"203.0.113.9","port":22}"#,
        )
        .await;
        assert_eq!(st, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(j["code"], "probe_failed");
        assert!(state.dyn_res.is_empty(), "探测失败不得落库");
        assert_eq!(state.dyn_res.count_by_owner("d@x.cn"), 0);
    }

    /// P4：动态资源经隧道解析层合并（agent 型直接命中 Agent 通道）。
    #[tokio::test]
    async fn p4_dyn_agent_resource_resolves_for_tunnel() {
        let state = p4_state("k");
        let now = crate::dynres::now_ms();
        let entry = crate::dynres::DynResource {
            id: "u-tunnel01".into(),
            owner: "t@x.cn".into(),
            label: "t".into(),
            mode: "agent".into(),
            backend: "-".into(),
            status: "pending".into(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        state.dyn_res.insert(entry.clone());
        state.resources.dyn_upsert(entry);
        assert!(matches!(
            state.resources.resolve_backend("u-tunnel01", &[]),
            crate::resources::BackendResolution::Agent { .. }
        ));
        // AgentToken 的 rid 绑定 + agent 角色可取本地后端（/agent/connect 校验路径）
        let tok = state.auth.issue_agent_token("agent-u-tunnel01", "u-tunnel01", 3600).unwrap();
        let claims = state.auth.validate_agent_token(&tok).unwrap();
        assert_eq!(
            state
                .resources
                .agent_backend_for("u-tunnel01", &claims.roles),
            Some("127.0.0.1:22".to_string())
        );
    }
}
