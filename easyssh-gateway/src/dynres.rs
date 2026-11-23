//! 动态资源注册表 + `POST /admin/resource`（R-20261122-004 P4 资源自助链路）。
//!
//! 方案第 4/6 节口径：
//! - 与静态 `[[resources]]` 并列的**运行时资源层**：内存 + 落盘
//!   `resources.dyn.toml`，重启恢复；动态资源 ID 统一 `u-` 前缀隔离命名空间；
//! - 条目含 `owner`（用户邮箱绑定）/`mode`/`status`；
//!   静态 + 动态两层合并解析（见 `resources.rs`）；
//! - `POST /admin/resource`（X-Admin-Key，供官网 PHP 回环）动作：
//!   `register`（direct：注册前 3s 试连探测并校验 SSH-2.0 banner 前 64 字节，
//!   拒绝私网/环回（SSRF 白名单）；agent：生成一次性 enrollment token，
//!   10 分钟、绑定 rid）/ `update`（label/backend，mode 不可改）/
//!   `delete`（摘除注册表 + 强断 Agent 通道）/ `status`（单条或全量，含在线态）；
//! - 每用户动态资源配额（[agent] max_resources_per_user，默认 3，仅限登记台数）；
//! - agent 型资源登记后 `pending`，经 `/agent/enroll` 换 AgentToken，
//!   Agent 反向注册（agent.rs `/agent/connect`）成功即翻 `online`；
//!   登记 7 天未上线 → 计算态 `cleanup_due`（待清理）。
//!
//! enrollment token 仅内存保存（网关重启即失效，重新登记即可），一次性消费。

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::audit::{check_admin_key_pub, admin_source_ip, AuditEvent};
use crate::resources::ResourceMode;
use crate::AppState;

/// 动态资源 ID 前缀（方案：动态资源 ID 前缀 `u-` 隔离命名空间）。
pub const DYN_ID_PREFIX: &str = "u-";

/// agent 型资源登记后未上线视为「待清理」的天数（方案 6.1 防滥用边界）。
pub const CLEANUP_AFTER_SECS: u64 = 7 * 24 * 3600;

/// enrollment token 有效期（10 分钟）。
pub const ENROLL_TOKEN_TTL_SECS: u64 = 600;
pub const ENROLL_TOKEN_TTL_MS: u64 = ENROLL_TOKEN_TTL_SECS * 1000;

/// direct 探测连接超时（3 秒，方案 6.1）。
pub const PROBE_CONNECT_TIMEOUT_MS: u64 = 3000;

/// direct 探测 banner 读取字节上限（前 64 字节，方案 6.1 SSH 特有补充）。
pub const PROBE_BANNER_BYTES: usize = 64;

// ---------------------------------------------------------------------
// 动态资源条目与注册表
// ---------------------------------------------------------------------

/// 一条动态（用户自助登记）资源。`backend`：direct = "ip:port"；agent = "-"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynResource {
    pub id: String,
    /// owner 邮箱（资源与用户绑定，update/delete 校验一致）。
    pub owner: String,
    pub label: String,
    /// "direct" | "agent"。
    pub mode: String,
    pub backend: String,
    /// pending / online / offline / revoked。
    pub status: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl DynResource {
    pub fn mode_parsed(&self) -> ResourceMode {
        ResourceMode::parse(&self.mode).unwrap_or(ResourceMode::Direct)
    }

    /// 计算态状态：agent 型登记后持续 `pending` 超过 7 天 → `cleanup_due`（待清理）。
    pub fn effective_status(&self, now_ms: u64) -> String {
        if self.mode_parsed() == ResourceMode::Agent
            && self.status == "pending"
            && now_ms.saturating_sub(self.created_at_ms) >= CLEANUP_AFTER_SECS * 1000
        {
            return "cleanup_due".to_string();
        }
        self.status.clone()
    }
}

/// 动态资源注册表：内存 BTreeMap + TOML 落盘（重启恢复）。
pub struct DynRegistry {
    map: RwLock<BTreeMap<String, DynResource>>,
    path: PathBuf,
}

#[derive(Serialize, Deserialize, Default)]
struct DynFile {
    #[serde(default, rename = "resource")]
    resources: Vec<DynResource>,
}

impl DynRegistry {
    /// 打开（或初始化）注册表：文件存在则恢复，缺失/损坏按空表起步。
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut map = BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            match toml::from_str::<DynFile>(&text) {
                Ok(file) => {
                    for r in file.resources {
                        if r.id.starts_with(DYN_ID_PREFIX) {
                            map.insert(r.id.clone(), r);
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, path = %path.display(), "动态资源文件解析失败，按空表起步");
                }
            }
        }
        let reg = Self { map: RwLock::new(map), path };
        let _ = reg.save();
        reg
    }

    pub fn len(&self) -> usize {
        self.map.read().expect("dyn registry poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: &str) -> Option<DynResource> {
        self.map.read().expect("dyn registry poisoned").get(id).cloned()
    }

    pub fn list(&self) -> Vec<DynResource> {
        self.map.read().expect("dyn registry poisoned").values().cloned().collect()
    }

    /// 某 owner 名下动态资源数（配额依据：仅限登记台数）。
    pub fn count_by_owner(&self, owner: &str) -> usize {
        self.map
            .read()
            .expect("dyn registry poisoned")
            .values()
            .filter(|r| r.owner == owner)
            .count()
    }

    fn persist(&self) -> Response {
        if let Err(err) = self.save() {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("动态资源落盘失败: {err}")).into_response();
        }
        (StatusCode::OK, axum::Json(serde_json::json!({}))).into_response()
    }

    /// 全量落盘（临时文件 + 原子替换，避免半写损坏）。
    pub fn save(&self) -> std::io::Result<()> {
        let snapshot: Vec<DynResource> =
            self.map.read().expect("dyn registry poisoned").values().cloned().collect();
        let text = toml::to_string_pretty(&DynFile { resources: snapshot })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// 注册（覆盖同 id）；调用方负责配额/探测/owner 校验。
    pub fn insert(&self, r: DynResource) {
        self.map.write().expect("dyn registry poisoned").insert(r.id.clone(), r);
        let _ = self.save();
    }

    /// 就地更新并落盘；id 不存在返回 None。
    pub fn update<F: FnOnce(&mut DynResource)>(&self, id: &str, f: F) -> Option<DynResource> {
        let mut map = self.map.write().expect("dyn registry poisoned");
        let r = map.get_mut(id)?;
        f(r);
        r.updated_at_ms = now_ms();
        let out = r.clone();
        drop(map);
        let _ = self.save();
        Some(out)
    }

    pub fn remove(&self, id: &str) -> Option<DynResource> {
        let out = self.map.write().expect("dyn registry poisoned").remove(id);
        if out.is_some() {
            let _ = self.save();
        }
        out
    }

    pub fn set_status(&self, id: &str, status: &str) -> bool {
        self.update(id, |r| r.status = status.to_string()).is_some()
    }

    /// 供测试/其他模块打开临时目录下的动态注册表。
    pub fn open_at(dir: &Path) -> DynRegistry {
        DynRegistry::open(dir.join("resources.dyn.toml"))
    }

    /// 测试用临时注册表（落盘到系统临时目录，每次调用独立文件，进程内隔离）。
    pub fn ephemeral() -> DynRegistry {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("easyssh-dynres-app-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        DynRegistry::open_at(&dir)
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// 生成动态资源 ID：`u-` + 12 位 hex（uuid v4 截断，碰撞概率可忽略）。
pub fn gen_resource_id() -> String {
    format!("{DYN_ID_PREFIX}{}", uuid::Uuid::new_v4().simple().to_string()[..12].to_string())
}

/// 生成一次性 enrollment token（uuid v4 simple ×2 = 64 hex，仅下发一次）。
pub fn gen_enroll_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------
// enrollment token 存储（一次性、10 分钟、绑定 rid/owner）
// ---------------------------------------------------------------------

#[derive(Clone)]
pub struct EnrollGrant {
    pub resource_id: String,
    pub owner: String,
    pub expires_ms: u64,
}

/// enrollment token 表（内存；重启失效）。`consume` 一次性取走。
#[derive(Default)]
pub struct EnrollStore {
    map: RwLock<std::collections::HashMap<String, EnrollGrant>>,
}

impl EnrollStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 签发并登记（自动清扫过期项）。
    pub fn issue(&self, resource_id: &str, owner: &str, now_ms: u64) -> (String, u64) {
        let token = gen_enroll_token();
        let expires_ms = now_ms + ENROLL_TOKEN_TTL_MS;
        let mut map = self.map.write().expect("enroll store poisoned");
        map.retain(|_, g| g.expires_ms > now_ms);
        map.insert(token.clone(), EnrollGrant {
            resource_id: resource_id.to_string(),
            owner: owner.to_string(),
            expires_ms,
        });
        (token, ENROLL_TOKEN_TTL_SECS)
    }

    /// 一次性消费：有效返回 (rid, owner)；过期/不存在返回 None（并顺手清扫）。
    pub fn consume(&self, token: &str, now_ms: u64) -> Option<EnrollGrant> {
        let mut map = self.map.write().expect("enroll store poisoned");
        map.retain(|_, g| g.expires_ms > now_ms);
        map.remove(token)
    }

    pub fn pending_len(&self) -> usize {
        self.map.read().expect("enroll store poisoned").len()
    }
}

// ---------------------------------------------------------------------
// direct 探测：3s 试连 + SSH-2.0 banner 前 64 字节
// ---------------------------------------------------------------------

/// direct 探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFault {
    /// SSRF 白名单拒绝（私网/环回/链路本地/DNS 主机名）。
    SsrfRejected(String),
    /// TCP 试连失败（3s 超时 / 拒连 / 不可达）。
    ConnectFailed(String),
    /// 已连通但 banner 非 SSH-2.0（防无关 TCP 服务误登记）。
    BannerMismatch(String),
}

impl std::fmt::Display for ProbeFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeFault::SsrfRejected(h) => write!(f, "host 被拒（仅允许公网 IP，禁止私网/环回/链路本地/DNS 主机名）: {h}"),
            ProbeFault::ConnectFailed(e) => write!(f, "试连失败（3s 超时或拒绝连接）: {e}"),
            ProbeFault::BannerMismatch(b) => write!(f, "banner 非 SSH-2.0，拒绝登记无关 TCP 服务: {b}"),
        }
    }
}

/// SSRF 防护（与网关回环白名单方向相反）：用户直连登记仅允许**公网 IP 字面量**；
/// 私网/环回/链路本地/组播/未指定地址与一切 DNS 主机名一律拒绝（网关不代查 DNS）。
pub fn is_public_backend(host: &str) -> Result<IpAddr, ProbeFault> {
    let host = host.trim();
    let ip: IpAddr = host.parse().map_err(|_| ProbeFault::SsrfRejected(host.to_string()))?;
    let reject = |why: &str| ProbeFault::SsrfRejected(format!("{host}（{why}）"));
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() || v4.is_private() || v4.is_link_local()
                || v4.is_unspecified() || v4.is_broadcast() || v4.is_multicast()
            {
                return Err(reject("私网/保留地址"));
            }
            let [a, b, _, _] = v4.octets();
            // 补充覆盖 is_private 未尽的 100.64.0.0/10（CGNAT）与 192.0.0.0/24 等保留段
            if a == 100 && (64..=127).contains(&b) {
                return Err(reject("保留地址"));
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return Err(reject("保留地址"));
            }
            // 唯一本地 fc00::/7 与链路本地 fe80::/10
            let seg = v6.segments()[0];
            if (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80 {
                return Err(reject("私网/链路本地地址"));
            }
        }
    }
    Ok(ip)
}

/// banner 校验：前 64 字节内出现 `SSH-2.0` 协议标识（RFC 4253 版本交换串前缀）。
pub fn banner_is_ssh2(bytes: &[u8]) -> bool {
    let head: Vec<u8> = bytes.iter().take(PROBE_BANNER_BYTES).copied().collect();
    let text = String::from_utf8_lossy(&head);
    text.starts_with("SSH-2.0") || text.starts_with("SSH-1.99")
}

/// TCP 试连（3s）+ 读前 64 字节校验 SSH-2.0 banner（不含 SSRF 判定，供本地 fixture 直测）。
async fn connect_and_read_banner(host: &str, port: u16) -> Result<(), ProbeFault> {
    let addr = format!("{host}:{port}");
    let stream = tokio::time::timeout(
        std::time::Duration::from_millis(PROBE_CONNECT_TIMEOUT_MS),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| ProbeFault::ConnectFailed("connect timeout".into()))?
    .map_err(|e| ProbeFault::ConnectFailed(e.to_string()))?;
    let mut stream = stream;
    let mut buf = vec![0u8; PROBE_BANNER_BYTES];
    let n = tokio::time::timeout(
        std::time::Duration::from_millis(PROBE_CONNECT_TIMEOUT_MS),
        tokio::io::AsyncReadExt::read(&mut stream, &mut buf),
    )
    .await
    .map_err(|_| ProbeFault::BannerMismatch("banner read timeout".into()))?
    .map_err(|e| ProbeFault::BannerMismatch(e.to_string()))?;
    let got = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    if banner_is_ssh2(&buf[..n]) {
        Ok(())
    } else {
        Err(ProbeFault::BannerMismatch(got.chars().take(48).collect()))
    }
}

/// direct 形态注册前探测：SSRF 白名单 → 3s TCP 试连 → SSH-2.0 banner 校验。
pub async fn probe_direct_backend(host: &str, port: u16) -> Result<(), ProbeFault> {
    is_public_backend(host)?;
    connect_and_read_banner(host, port).await
}

// ---------------------------------------------------------------------
// POST /admin/resource（X-Admin-Key；官网 PHP 回环调用）
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AdminResourceRequest {
    /// register / update / delete / status。
    pub action: String,
    /// 资源 ID（update/delete/status 必填；register 缺省自动生成 u- id）。
    #[serde(default)]
    pub id: Option<String>,
    /// owner 邮箱（register 必填；update/delete 必须与登记 owner 一致）。
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    /// register 用；update 提供 mode 即拒绝（mode 不可改）。
    #[serde(default)]
    pub mode: Option<String>,
    /// direct 形态后端（公网 IP 字面量）。
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}

fn owner_email_valid(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 254
        && owner.contains('@')
        && !owner.contains(char::is_whitespace)
        && owner.chars().all(|c| c.is_ascii_graphic())
}

fn label_or_default(label: &Option<String>, fallback: &str) -> String {
    match label {
        Some(l) if !l.trim().is_empty() => l.trim().chars().take(64).collect(),
        _ => fallback.to_string(),
    }
}

/// 单条资源对外视图（管理面可见 backend；不暴露 enrollment token）。
fn resource_view(r: &DynResource, state: &AppState, now: u64) -> serde_json::Value {
    serde_json::json!({
        "id": r.id,
        "owner": r.owner,
        "label": r.label,
        "mode": r.mode,
        "backend": r.backend,
        "status": r.effective_status(now),
        "agent_online": state.agent_hub.is_online(&r.id),
        "created_at": r.created_at_ms,
        "updated_at": r.updated_at_ms,
    })
}

async fn admin_resource(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AdminResourceRequest>,
) -> Response {
    if let Some(resp) = check_admin_key_pub(&state, &headers) {
        return resp;
    }
    let now = now_ms();
    match req.action.trim().to_ascii_lowercase().as_str() {
        "register" => admin_resource_register(state, headers, req, now).await,
        "update" => admin_resource_update(state, headers, req, now),
        "delete" => admin_resource_delete(state, headers, req),
        "detach" => admin_resource_detach(state, headers, req),
        "status" => admin_resource_status(state, req, now),
        other => (
            StatusCode::BAD_REQUEST,
            format!("action 非法（register/update/delete/status）: {other}"),
        )
            .into_response(),
    }
}

async fn admin_resource_register(
    state: Arc<AppState>,
    headers: HeaderMap,
    req: AdminResourceRequest,
    now: u64,
) -> Response {
    let owner = req.owner.as_deref().map(str::trim).unwrap_or_default().to_ascii_lowercase();
    if !owner_email_valid(&owner) {
        return (StatusCode::BAD_REQUEST, "owner 邮箱非法").into_response();
    }
    // 配额：每用户动态资源台数（默认 3；仅限登记台数，token 签发不限次）
    let quota = state.agent_cfg.max_resources_per_user();
    if state.dyn_res.count_by_owner(&owner) >= quota {
        let mut ev = AuditEvent::new(&owner, Some("user"), "admin_resource_register", "-", &admin_source_ip(&headers));
        ev.error = Some(format!("quota_exceeded limit={quota}"));
        state.audit.record(ev);
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("已达到每用户登记上限（{quota} 台）；请先删除不再使用的资源"),
                "code": "quota_exceeded",
            })),
        )
            .into_response();
    }
    let mode = ResourceMode::parse(req.mode.as_deref().map(str::trim).unwrap_or("direct"));
    let Some(mode) = mode else {
        return (StatusCode::BAD_REQUEST, "mode 非法（direct/agent）").into_response();
    };
    let id = gen_resource_id();
    match mode {
        ResourceMode::Direct => {
            let host = req.host.as_deref().map(str::trim).unwrap_or_default().to_string();
            let port = req.port.unwrap_or(22);
            if let Err(fault) = probe_direct_backend(&host, port).await {
                let mut ev = AuditEvent::new(&owner, Some("user"), "admin_resource_register", &id, &admin_source_ip(&headers));
                ev.error = Some(format!("probe_failed: {fault}"));
                state.audit.record(ev);
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({ "ok": false, "message": fault.to_string(), "code": "probe_failed" })),
                )
                    .into_response();
            }
            let label = label_or_default(&req.label, &host);
            let entry = DynResource {
                id: id.clone(),
                owner: owner.clone(),
                label,
                mode: "direct".into(),
                backend: format!("{host}:{port}"),
                status: "online".into(),
                created_at_ms: now,
                updated_at_ms: now,
            };
            state.dyn_res.insert(entry.clone());
            // 同步隧道解析动态层（静态+动态合并）
            state.resources.dyn_upsert(entry.clone());
            state.audit.record(AuditEvent::new(
                &owner, Some("user"), "admin_resource_register", &id, &admin_source_ip(&headers),
            ));
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "ok": true,
                    "message": "direct 资源已登记（探测通过：SSH-2.0 banner）",
                    "resource": resource_view(&entry, &state, now),
                })),
            )
                .into_response()
        }
        ResourceMode::Agent => {
            let label = label_or_default(&req.label, &id);
            let entry = DynResource {
                id: id.clone(),
                owner: owner.clone(),
                label,
                mode: "agent".into(),
                backend: "-".into(),
                status: "pending".into(),
                created_at_ms: now,
                updated_at_ms: now,
            };
            state.dyn_res.insert(entry.clone());
            state.resources.dyn_upsert(entry.clone());
            let (token, ttl) = state.enroll.issue(&id, &owner, now);
            state.audit.record(AuditEvent::new(
                &owner, Some("user"), "admin_resource_register", &id, &admin_source_ip(&headers),
            ));
            (
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "ok": true,
                    "message": "agent 资源已登记（pending）；请在 10 分钟内用 enrollment token 于目标机执行 easyssh-agent --enroll",
                    "resource": resource_view(&entry, &state, now),
                    "enrollment_token": token,
                    "enrollment_expires_in": ttl,
                })),
            )
                .into_response()
        }
    }
}

fn require_owner_match(
    state: &AppState,
    headers: &HeaderMap,
    event: &str,
    entry: Option<DynResource>,
    req_owner: Option<&str>,
) -> Result<DynResource, Response> {
    let Some(entry) = entry else {
        return Err((StatusCode::NOT_FOUND, "资源不存在或已摘除").into_response());
    };
    let owner = req_owner.map(|o| o.trim().to_ascii_lowercase()).unwrap_or_default();
    if owner != entry.owner {
        let mut ev = AuditEvent::new(&owner, Some("user"), event, &entry.id, &admin_source_ip(headers));
        ev.error = Some("owner_mismatch".into());
        state.audit.record(ev);
        return Err((StatusCode::FORBIDDEN, "owner 与资源登记人不一致，操作被拒绝").into_response());
    }
    Ok(entry)
}

fn admin_resource_update(
    state: Arc<AppState>,
    headers: HeaderMap,
    req: AdminResourceRequest,
    _now: u64,
) -> Response {
    let id = req.id.as_deref().map(str::trim).unwrap_or_default().to_string();
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "id 必填").into_response();
    }
    let entry = require_owner_match(&state, &headers, "admin_resource_update", state.dyn_res.get(&id), req.owner.as_deref());
    let entry = match entry {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    // mode 不可改（方案 ③：update_resource 仅 label/backend）
    if let Some(m) = req.mode.as_deref() {
        if ResourceMode::parse(m) != Some(entry.mode_parsed()) {
            return (StatusCode::BAD_REQUEST, "mode 不可修改（删除后重新登记）").into_response();
        }
    }
    // backend 变更仅 direct 形态允许；变更后同步刷新探测标记由后续 status 探测兜底
    let new_backend = if entry.mode_parsed() == ResourceMode::Direct {
        match (&req.host, req.port) {
            (Some(h), _) if !h.trim().is_empty() => {
                let host = h.trim();
                let port = req.port.unwrap_or_else(|| {
                    crate::resources::parse_host_port(&entry.backend).map(|(_, p)| p).unwrap_or(22)
                });
                if let Err(fault) = is_public_backend(host) {
                    return (StatusCode::BAD_REQUEST, fault.to_string()).into_response();
                }
                format!("{host}:{port}")
            }
            _ => entry.backend.clone(),
        }
    } else {
        "-".to_string()
    };
    let label = req.label.clone().map(|l| l.trim().chars().take(64).collect::<String>());
    let updated = state.dyn_res.update(&id, |r| {
        if let Some(l) = label.clone() {
            if !l.is_empty() {
                r.label = l;
            }
        }
        r.backend = new_backend.clone();
    });
    let updated = match updated {
        Some(u) => u,
        None => return (StatusCode::NOT_FOUND, "资源不存在或已摘除").into_response(),
    };
    state.resources.dyn_upsert(updated.clone());
    let mut ev = AuditEvent::new(&updated.owner, Some("user"), "admin_resource_update", &id, &admin_source_ip(&headers));
    ev.error = Some(format!("backend={}", updated.backend));
    state.audit.record(ev);
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "ok": true,
            "message": "资源已更新",
            "resource": resource_view(&updated, &state, now_ms()),
        })),
    )
        .into_response()
}

fn admin_resource_delete(state: Arc<AppState>, headers: HeaderMap, req: AdminResourceRequest) -> Response {
    let id = req.id.as_deref().map(str::trim).unwrap_or_default().to_string();
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "id 必填").into_response();
    }
    let entry = require_owner_match(&state, &headers, "admin_resource_delete", state.dyn_res.get(&id), req.owner.as_deref());
    let entry = match entry {
        Ok(e) => e,
        Err(resp) => return resp,
    };
    // 摘除注册表 + 强断 Agent 通道（在线 Agent 下一次心跳/写失败自然退出）
    state.dyn_res.remove(&id);
    state.resources.dyn_remove(&id);
    if let Some(slot) = state.agent_hub.detach(&id) {
        let _ = slot.push(crate::agent::Envelope::new(crate::agent::Op::Close, 0, Vec::new()));
    }
    state.audit.record(AuditEvent::new(
        &entry.owner, Some("user"), "admin_resource_delete", &id, &admin_source_ip(&headers),
    ));
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "ok": true, "message": "资源已摘除（动态注册表 + Agent 通道）", "id": id })),
    )
        .into_response()
}

/// 强制摘除 Agent 通道（站长后台「摘除 Agent」；不动注册表登记本身，
/// 目标机 Agent 下一次重连即恢复在线）。
fn admin_resource_detach(state: Arc<AppState>, headers: HeaderMap, req: AdminResourceRequest) -> Response {
    let id = req.id.as_deref().map(str::trim).unwrap_or_default().to_string();
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "id 必填").into_response();
    }
    let Some(entry) = state.dyn_res.get(&id) else {
        return (StatusCode::NOT_FOUND, "资源不存在或已摘除").into_response();
    };
    let detached = state.agent_hub.detach(&id).is_some();
    if detached {
        // 联动态：offline（重连即恢复 online）
        state.dyn_res.set_status(&id, "offline");
        state.resources.dyn_set_status(&id, "offline");
    }
    state.audit.record(AuditEvent::new(
        &entry.owner, Some("admin"), "admin_resource_detach", &id, &admin_source_ip(&headers),
    ));
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "ok": true,
            "message": if detached { "Agent 通道已强制摘除" } else { "该资源当前无在线 Agent" },
            "id": id,
            "detached": detached,
        })),
    )
        .into_response()
}

fn admin_resource_status(state: Arc<AppState>, req: AdminResourceRequest, now: u64) -> Response {    let now = now_ms();
    let view = |r: &DynResource| {
        let mut v = resource_view(r, &state, now);
        // pending 动态资源若存在未消费 enrollment token，顺带回报剩余有效期
        v
    };
    if let Some(id) = req.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return match state.dyn_res.get(id) {
            Some(r) => (
                StatusCode::OK,
                axum::Json(serde_json::json!({ "ok": true, "resource": view(&r) })),
            )
                .into_response(),
            None => (StatusCode::NOT_FOUND, "资源不存在或已摘除").into_response(),
        };
    }
    let resources: Vec<serde_json::Value> = state.dyn_res.list().iter().map(view).collect();
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "ok": true, "resources": resources })),
    )
        .into_response()
}

/// 只读探针（运维排障用）：GET /admin/resource → 动态资源全量。
async fn admin_resource_list(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(resp) = check_admin_key_pub(&state, &headers) {
        return resp;
    }
    let now = now_ms();
    let resources: Vec<serde_json::Value> =
        state.dyn_res.list().iter().map(|r| resource_view(r, &state, now)).collect();
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "ok": true, "resources": resources })),
    )
        .into_response()
}

pub fn router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/resource", post(admin_resource).get(admin_resource_list))
        .with_state(state)
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn sample(id: &str, owner: &str, mode: &str, status: &str, created: u64) -> DynResource {
        DynResource {
            id: id.to_string(),
            owner: owner.to_string(),
            label: id.to_string(),
            mode: mode.to_string(),
            backend: if mode == "agent" { "-".into() } else { "203.0.113.9:22".into() },
            status: status.to_string(),
            created_at_ms: created,
            updated_at_ms: created,
        }
    }

    fn tmp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("easyssh-dynres-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("resources.dyn.toml")
    }

    #[test]
    fn public_backend_whitelist_rejects_private_and_dns() {
        for bad in [
            "127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.9", "172.31.255.1",
            "169.254.1.1", "100.64.0.1", "0.0.0.0", "255.255.255.255", "239.1.1.1",
            "example.com", "", "203.0.113.9:22",
            "::1", "fe80::1", "fc00::1", "fd12::1", "::",
        ] {
            assert!(is_public_backend(bad).is_err(), "{bad} 应被 SSRF 白名单拒绝");
        }
        for good in ["203.0.113.9", "8.8.8.8", "2606:4700::1111"] {
            assert!(is_public_backend(good).is_ok(), "{good} 应为公网放行");
        }
    }

    #[test]
    fn banner_check_requires_ssh2_prefix() {
        assert!(banner_is_ssh2(b"SSH-2.0-OpenSSH_9.6\r\n"));
        assert!(banner_is_ssh2(b"SSH-1.99-X\r\n"));
        assert!(!banner_is_ssh2(b"HTTP/1.1 400 Bad Request\r\n"));
        assert!(!banner_is_ssh2(b"220 smtp.example.com ESMTP\r\n"));
        assert!(!banner_is_ssh2(b""));
    }

    #[tokio::test]
    async fn probe_accepts_ssh_banner_and_rejects_other_services() {
        // 假 SSH：回 SSH-2.0 banner（本地 TcpListener fixture）
        let ok = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let ok_port = ok.local_addr().unwrap().port();
        let t = tokio::spawn(async move {
            let (mut s, _) = ok.accept().await.expect("accept");
            let _ = s.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").await;
        });
        assert_eq!(connect_and_read_banner("127.0.0.1", ok_port).await, Ok(()));
        t.abort();

        // 假非 SSH 服务：回 HTTP banner → BannerMismatch
        let bad = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let bad_port = bad.local_addr().unwrap().port();
        let t2 = tokio::spawn(async move {
            let (mut s, _) = bad.accept().await.expect("accept");
            let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n").await;
        });
        match connect_and_read_banner("127.0.0.1", bad_port).await {
            Err(ProbeFault::BannerMismatch(b)) => assert!(b.contains("HTTP"), "{b}"),
            other => panic!("HTTP 服务应命中 BannerMismatch，实际 {other:?}"),
        }
        t2.abort();

        // 连接失败：无监听端口 → ConnectFailed
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);
        match connect_and_read_banner("127.0.0.1", dead_port).await {
            Err(ProbeFault::ConnectFailed(_)) => {}
            other => panic!("拒连应命中 ConnectFailed，实际 {other:?}"),
        }

        // probe_direct_backend 组合层：loopback 先命中 SSRF 白名单
        let ok2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let ok2_port = ok2.local_addr().unwrap().port();
        let t3 = tokio::spawn(async move {
            let (mut s, _) = ok2.accept().await.expect("accept");
            let _ = s.write_all(b"SSH-2.0-fixture\r\n").await;
        });
        match probe_direct_backend("127.0.0.1", ok2_port).await {
            Err(ProbeFault::SsrfRejected(_)) => {}
            other => panic!("loopback 应命中 SSRF 白名单，实际 {other:?}"),
        }
        t3.abort();
    }

    #[test]
    fn registry_persists_and_restores_across_reopen() {
        let path = tmp_path("persist");
        let _ = std::fs::remove_file(&path);
        {
            let reg = DynRegistry::open(&path);
            reg.insert(sample("u-aaa111", "a@x.cn", "direct", "online", now_ms()));
            reg.insert(sample("u-bbb222", "b@x.cn", "agent", "pending", now_ms()));
            reg.remove("u-bbb222");
        }
        let reg2 = DynRegistry::open(&path);
        assert_eq!(reg2.len(), 1);
        let r = reg2.get("u-aaa111").expect("restored");
        assert_eq!(r.mode, "direct");
        assert_eq!(r.backend, "203.0.113.9:22");
        assert_eq!(r.owner, "a@x.cn");
        // 损坏文件按空表起步
        std::fs::write(&path, "not [valid toml").expect("write");
        let reg3 = DynRegistry::open(&path);
        assert!(reg3.is_empty());
    }

    #[test]
    fn cleanup_due_computed_for_stale_pending_agent() {
        let now = now_ms();
        let stale = sample("u-old9999", "o@x.cn", "agent", "pending", now - CLEANUP_AFTER_SECS * 1000 - 1);
        let fresh = sample("u-new0001", "o@x.cn", "agent", "pending", now);
        let online = sample("u-live001", "o@x.cn", "agent", "online", now - CLEANUP_AFTER_SECS * 1000 - 1);
        let direct_old = sample("u-dir0001", "o@x.cn", "direct", "online", now - CLEANUP_AFTER_SECS * 1000 - 1);
        assert_eq!(stale.effective_status(now), "cleanup_due");
        assert_eq!(fresh.effective_status(now), "pending");
        assert_eq!(online.effective_status(now), "online");
        assert_eq!(direct_old.effective_status(now), "online");
    }

    #[test]
    fn enroll_token_one_shot_and_expiry() {
        let store = EnrollStore::new();
        let now = now_ms();
        let (tok, ttl) = store.issue("u-abc", "u@x.cn", now);
        assert_eq!(ttl, ENROLL_TOKEN_TTL_SECS);
        assert_eq!(store.pending_len(), 1);
        let g = store.consume(&tok, now + 1000).expect("consume");
        assert_eq!(g.resource_id, "u-abc");
        assert_eq!(g.owner, "u@x.cn");
        assert!(store.consume(&tok, now + 2000).is_none(), "一次性：二次消费为空");
        // 过期 token 不可消费
        let (tok2, _) = store.issue("u-def", "u@x.cn", now);
        assert!(store.consume(&tok2, now + ENROLL_TOKEN_TTL_MS + 1).is_none());
        // 未过期可消费
        let (tok3, _) = store.issue("u-ghi", "u@x.cn", now);
        assert!(store.consume(&tok3, now + ENROLL_TOKEN_TTL_MS - 1).is_some());
    }

    #[test]
    fn owner_quota_counting() {
        let path = tmp_path("quota");
        let _ = std::fs::remove_file(&path);
        let reg = DynRegistry::open(&path);
        let now = now_ms();
        reg.insert(sample("u-a1", "q@x.cn", "direct", "online", now));
        reg.insert(sample("u-a2", "q@x.cn", "agent", "pending", now));
        reg.insert(sample("u-b1", "r@x.cn", "direct", "online", now));
        assert_eq!(reg.count_by_owner("q@x.cn"), 2);
        assert_eq!(reg.count_by_owner("r@x.cn"), 1);
        assert_eq!(reg.count_by_owner("none@x.cn"), 0);
    }

    #[test]
    fn id_prefix_and_token_shape() {
        let id = gen_resource_id();
        assert!(id.starts_with(DYN_ID_PREFIX), "动态 ID 必须 u- 前缀: {id}");
        assert_eq!(id.len(), DYN_ID_PREFIX.len() + 12);
        let tok = gen_enroll_token();
        assert_eq!(tok.len(), 64);
        assert!(tok.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn owner_email_validation() {
        assert!(owner_email_valid("user@example.com"));
        assert!(owner_email_valid("USER+x@sub.example.cn"));
        assert!(!owner_email_valid(""));
        assert!(!owner_email_valid("no-at-mark"));
        assert!(!owner_email_valid("a b@x.cn"));
        assert!(!owner_email_valid(&"x".repeat(300)));
    }

    #[test]
    fn label_fallback_and_clamp() {
        assert_eq!(label_or_default(&None, "fb"), "fb");
        assert_eq!(label_or_default(&Some("  ".into()), "fb"), "fb");
        assert_eq!(label_or_default(&Some(" 我的机器 ".into()), "fb"), "我的机器");
        let long = "长".repeat(100);
        assert_eq!(label_or_default(&Some(long), "fb").chars().count(), 64);
    }
}
