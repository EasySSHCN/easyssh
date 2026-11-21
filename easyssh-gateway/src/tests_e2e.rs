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
}
