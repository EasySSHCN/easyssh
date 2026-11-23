//! EasySSH 云网关入口。
//!
//! 职责：加载 config.toml（server / auth / audit / admin / tls / users /
//! oidc / [[resources]]，缺失时回退环境变量与默认值）、初始化资源注册表与
//! 审计环形缓冲、解析 dev_mode CIDR 白名单（fail-fast）、挂载路由并启动
//! axum HTTP/WS 服务。网关本体默认绑定 **127.0.0.1:8443 回环**；配置
//! [tls] cert_path/key_path（自签证书即可）时直跑 HTTPS/WSS。

mod agent;
mod audit;
mod auth;
mod cidr;
mod config;
mod console;
mod dynres;
mod policy;
mod resources;
mod tunnel;
/// 端到端集成测试（仅 #[cfg(test)] 编译）：占位登录四用例 + 隧道全链路。
#[cfg(test)]
mod tests_e2e;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use tracing_subscriber::EnvFilter;

/// 网关共享状态：认证状态 + 审计缓冲 + 资源注册表 + 管理密钥 + 策略矩阵 +
/// dev_mode 占位闸门。
pub struct AppState {
    pub auth: auth::AuthState,
    pub audit: audit::AuditLog,
    pub resources: resources::ResourceRegistry,
    /// 管理接口共享密钥（None = 管理端点禁用，/admin/* 返回 503）
    pub admin_key: Option<Arc<String>>,
    /// 策略矩阵（可热加载；缺省 deny by default、admin 全通）
    pub policy: policy::PolicyHandle,
    /// 用户 -> TOTP Base32 密钥（空表 = 无用户要求 OTP 二次认证）
    pub totp_secrets: std::collections::BTreeMap<String, String>,
    /// OIDC 运行态（None = 未配置 [oidc]，真实 code 换票不可用）
    pub oidc: Option<Arc<auth::OidcState>>,
    /// R-20261120-006：dev_mode 总闸（默认 false，生产口径 false）
    pub dev_mode: bool,
    /// R-20261120-006：dev_mode 占位登录来源 IP CIDR 白名单
    pub dev_cidrs: Arc<Vec<cidr::Cidr>>,
    /// R-20261122-004：Agent 反向连接注册表（resource_id -> AgentSlot）
    pub agent_hub: agent::AgentHub,
    /// R-20261122-004：[agent] 配置段
    pub agent_cfg: config::AgentConfig,
    /// R-20261122-004 P2：客户端会话号分配器（每条 Agent 连接内唯一）
    pub agent_sessions: agent::SessionSeq,
    /// R-20261122-004 P4：动态资源注册表（用户自助登记，落盘 resources.dyn.toml）
    pub dyn_res: dynres::DynRegistry,
    /// R-20261122-004 P4：一次性 enrollment token 表（10 分钟、绑定 rid）
    pub enroll: dynres::EnrollStore,
}

/// 策略热加载轮询间隔（Windows 无 SIGHUP，统一用 mtime 轮询）。
const POLICY_RELOAD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// 后台热加载任务：每 30s 检查 config.toml mtime，变更时重载 [policy] 段。
/// 成功记录 policy_reload 审计；失败保留旧策略并记录审计事件。
async fn policy_reload_task(config_path: String, state: Arc<AppState>) {
    let mut last_mtime: Option<std::time::SystemTime> =
        std::fs::metadata(&config_path).ok().and_then(|m| m.modified().ok());
    loop {
        tokio::time::sleep(POLICY_RELOAD_INTERVAL).await;
        let mtime = std::fs::metadata(&config_path).ok().and_then(|m| m.modified().ok());
        if mtime == last_mtime {
            continue;
        }
        last_mtime = mtime;
        match state.policy.reload_from_file(&config_path) {
            Ok(_) => {
                tracing::info!("policy matrix hot-reloaded");
                state.audit.record(audit::AuditEvent::new(
                    "-",
                    None,
                    "policy_reload",
                    "config",
                    "-",
                ));
            }
            Err(err) => {
                tracing::warn!(%err, "policy hot-reload failed, keeping previous matrix");
                let mut ev = audit::AuditEvent::new("-", None, "policy_reload_failed", "config", "-");
                ev.error = Some(err);
                state.audit.record(ev);
            }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // 配置：优先 config.toml（EASYSSH_CONFIG 指定路径，缺省 ./config.toml）。
    let config_path =
        std::env::var("EASYSSH_CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    let cfg = config::GatewayConfig::load_from_path(&config_path).unwrap_or_default();

    let jwt_secret = cfg
        .auth
        .jwt_secret
        .or_else(|| std::env::var("EASYSSH_JWT_SECRET").ok())
        .unwrap_or_else(|| "dev-secret-do-not-use".to_string());
    // 默认绑定回环（R-20261120：网关本体 127.0.0.1:8443，公网经反代转发）
    let listen_addr = cfg
        .server
        .listen_addr
        .or_else(|| std::env::var("EASYSSH_LISTEN").ok())
        .unwrap_or_else(|| "127.0.0.1:8443".to_string());
    let ttl = cfg.auth.token_ttl_seconds.unwrap_or(auth::DEFAULT_TOKEN_TTL);
    let capacity = cfg.audit.ring_capacity.unwrap_or(4096).max(1);
    // 管理密钥：config.toml [admin].admin_key 优先，缺失回退环境变量；
    // 均未配置则 /admin/* 管理端点保持禁用（返回 503）。
    let admin_key = cfg
        .admin
        .admin_key
        .or_else(|| std::env::var("EASYSSH_ADMIN_KEY").ok())
        .filter(|k| !k.trim().is_empty())
        .map(|k| Arc::new(k));

    let registry = resources::ResourceRegistry::from_configs(&cfg.resources);
    tracing::info!(count = cfg.resources.len(), "resource registry loaded");

    // P4：动态资源注册表 —— 落盘 resources.dyn.toml（config 同目录，环境变量
    // EASYSSH_DYN_RESOURCES 覆盖），启动恢复；静态+动态两层合并解析。
    let dyn_path = std::env::var("EASYSSH_DYN_RESOURCES")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let cfg_file = std::path::Path::new(&config_path);
            let dir = cfg_file.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            dir.join("resources.dyn.toml")
        });
    let dyn_res = dynres::DynRegistry::open(&dyn_path);
    // 同步进资源注册表动态层（隧道解析合并）
    registry.dyn_replace_all(dyn_res.list());
    tracing::info!(count = dyn_res.len(), path = %dyn_path.display(), "dynamic resource registry loaded");

    // 策略矩阵：config.toml [policy] 段编译；解析失败回退默认矩阵
    // （deny by default、admin 全通）并记录 warn。
    let policy = match policy::PolicyMatrix::from_config(&cfg.policy) {
        Ok(m) => policy::PolicyHandle::from_matrix(m),
        Err(err) => {
            tracing::warn!(%err, "policy matrix parse failed, using default matrix");
            policy::PolicyHandle::default()
        }
    };

    // TOTP 用户表：[users.<user>].totp_secret 优先，
    // 环境变量 EASYSSH_TOTP_SECRET_<USER大写> 补充/覆盖。
    let mut totp_secrets = std::collections::BTreeMap::new();
    for (user, uc) in &cfg.users {
        if let Some(s) = uc.totp_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            totp_secrets.insert(user.clone(), s.to_string());
        }
    }
    for (key, val) in std::env::vars() {
        if let Some(user) = key
            .strip_prefix("EASYSSH_TOTP_SECRET_")
            .filter(|k| !k.trim().is_empty())
        {
            totp_secrets.insert(user.to_string(), val);
        }
    }
    tracing::info!(users = totp_secrets.len(), "totp second-factor users loaded");

    // [oidc] 段 —— 配置齐全才启用真实 OIDC code 换票主路径。
    let oidc = if cfg.oidc.enabled() {
        tracing::info!(
            issuer = cfg.oidc.issuer.as_deref().unwrap_or(""),
            "OIDC enabled (authorization code flow)"
        );
        Some(Arc::new(auth::OidcState::new(
            cfg.oidc.clone(),
            Arc::new(auth::HttpIdpClient::new()),
        )))
    } else {
        tracing::info!("OIDC not configured");
        None
    };

    // R-20261120-006：dev_mode CIDR 白名单解析 —— fail-fast（任一条非法
    // 直接退出，避免静默放行）。dev_mode=false（生产口径）白名单不参与判定。
    let dev_mode = cfg.auth.dev_mode;
    let dev_cidrs = match cidr::parse_cidrs(&cfg.auth.dev_mode_allow_cidrs) {
        Ok(list) => Arc::new(list),
        Err(err) => {
            eprintln!("config [auth] dev_mode_allow_cidrs 解析失败: {err}");
            std::process::exit(2);
        }
    };
    if dev_mode {
        tracing::warn!(
            cidrs = cfg.auth.dev_mode_allow_cidrs.len(),
            "DEV_MODE ENABLED: placeholder login allowed for allowlisted source IPs only"
        );
    }

    let state = Arc::new(AppState {
        auth: auth::AuthState::new(jwt_secret, ttl),
        audit: audit::AuditLog::new(capacity),
        resources: registry,
        admin_key,
        policy,
        totp_secrets,
        oidc,
        dev_mode,
        dev_cidrs,
        agent_hub: agent::AgentHub::new(),
        agent_cfg: cfg.agent.clone(),
        agent_sessions: agent::SessionSeq::new(),
        dyn_res,
        enroll: dynres::EnrollStore::new(),
    });

    let app = Router::new()
        .merge(auth::router(Arc::clone(&state)))
        .merge(console::router(Arc::clone(&state)))
        .merge(tunnel::router(Arc::clone(&state)))
        .merge(agent::router())
        .merge(dynres::router(Arc::clone(&state)))
        .merge(audit::admin_router(Arc::clone(&state)))
        .with_state(state.clone());

    // Agent 心跳判死清扫任务（R-20261122-004）：每半个心跳周期扫一次。
    if state.agent_cfg.enabled {
        let sweep_state = Arc::clone(&state);
        tokio::spawn(async move {
            let window_ms = sweep_state.agent_cfg.silence_window_secs() * 1000;
            let mut tick =
                tokio::time::interval(std::time::Duration::from_millis((window_ms / 2).max(1000)));
            loop {
                tick.tick().await;
                let removed = sweep_state.agent_hub.prune_silent(agent::now_ms(), window_ms);
                for rid in removed {
                    tracing::warn!(resource_id = %rid, "agent silent timeout, detached");
                    // P4：静默判死同样联动动态资源 offline 态
                    sweep_state.resources.dyn_set_status(&rid, "offline");
                    sweep_state.dyn_res.set_status(&rid, "offline");
                    sweep_state.audit.record(audit::AuditEvent::new(
                        "-",
                        Some("agent"),
                        "agent_offline",
                        &rid,
                        "-",
                    ));
                }
            }
        });
    }

    // 策略热加载轮询任务
    tokio::spawn(policy_reload_task(config_path, state));

    let addr: SocketAddr = listen_addr
        .parse()
        .expect("listen_addr 必须是合法的 SocketAddr，例如 127.0.0.1:8443");

    tracing::info!(%addr, "EasySSH gateway listening");
    // 注入 ConnectInfo<SocketAddr>：登录闸门与隧道审计需要客户端来源 IP。
    // [tls] cert_path/key_path → axum-server rustls 直跑 HTTPS/WSS（自签证书
    // 即可）；未配置 → 保持 HTTP（由反代终止 TLS 的部署形态）。
    let make_svc = app.into_make_service_with_connect_info::<SocketAddr>();
    if cfg.tls.enabled() {
        let cert_path = cfg.tls.cert_path.clone().unwrap();
        let key_path = cfg.tls.key_path.clone().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls_cfg = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path)
            .await
            .unwrap_or_else(|e| {
                panic!("TLS 证书加载失败 (cert={cert_path}, key={key_path}): {e}")
            });
        tracing::info!("TLS enabled (HTTPS/WSS), cert={cert_path}");
        axum_server::bind_rustls(addr, tls_cfg)
            .serve(make_svc)
            .await
            .expect("HTTPS 服务异常退出");
    } else {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("监听端口失败");
        axum::serve(listener, make_svc)
            .await
            .expect("HTTP 服务异常退出");
    }
}
