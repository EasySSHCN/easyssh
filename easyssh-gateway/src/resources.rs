//! 资源注册表模块：内网 SSH 资源目录 + 后端地址解析 + 角色级授权 + SSRF 防护。
//!
//! config.toml `[[resources]]`：id / name / host / port（端口缺省 22）。
//! 后端地址解析优先级：
//! 1. 注册表条目（并按可选 `allowed_roles` 与 JWT claims.roles 求交集授权，
//!    缺省 = 所有角色可用；无交集返回 `BackendResolution::Forbidden`）；
//! 2. resource_id 本身是显式 `"host:port"`，且主机通过 [`is_allowed_backend`]
//!    私网白名单校验（防 SSRF 任意探测公网/DNS 目标）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::RwLock;

/// config.toml `[[resources]]` 条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// 资源 ID（客户端 /tunnel/{resource_id} 使用的标识）
    pub id: String,
    /// 展示名称
    pub name: String,
    /// 后端 SSH 主机（IP 字面量；网关不代查 DNS）
    pub host: String,
    /// 后端 SSH 端口（缺省 22）
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// 允许访问该资源的角色列表（与 JWT claims.roles 求交集）；
    /// 缺省（None 或空数组）= 所有角色可用。
    #[serde(default)]
    pub allowed_roles: Option<Vec<String>>,
}

fn default_ssh_port() -> u16 {
    22
}

impl ResourceConfig {
    /// 后端地址 "host:port"（仅注册表内部可见，不通过 API 暴露）。
    pub fn backend(&self) -> String {
        format!("{}:{}", self.host.trim(), self.port)
    }
}

/// 注册表内部条目：展示名 + 后端地址 + 角色授权。
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub name: String,
    pub backend: String,
    pub allowed_roles: Option<Vec<String>>,
}

/// resolve_backend 的授权判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendResolution {
    /// 授权通过，返回后端地址 "host:port"。
    Allowed(String),
    /// 资源存在但当前角色不被 allowed_roles 允许（映射 403 tunnel_forbidden）。
    Forbidden,
    /// 资源未注册 / 后端地址非法 / 显式直连未过 SSRF 白名单（映射 404）。
    NotFound,
}

/// 对外资源条目（GET /api/resources）：只含 id/name，不含 host/port。
#[derive(Debug, Clone, Serialize)]
pub struct ResourceInfo {
    pub id: String,
    pub name: String,
}

/// 资源注册表：id -> ResourceEntry。
#[derive(Default)]
pub struct ResourceRegistry {
    map: RwLock<HashMap<String, ResourceEntry>>,
}

impl ResourceRegistry {
    pub fn from_configs(configs: &[ResourceConfig]) -> Self {
        let map = configs
            .iter()
            .map(|r| {
                (
                    r.id.clone(),
                    ResourceEntry {
                        name: r.name.clone(),
                        backend: r.backend(),
                        allowed_roles: r.allowed_roles.clone(),
                    },
                )
            })
            .collect();
        Self { map: RwLock::new(map) }
    }

    /// 简易注册（测试用）：展示名回退为 id，角色授权缺省全放行。
    pub fn register(&self, id: impl Into<String>, backend: impl Into<String>) {
        let id = id.into();
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(
                id.clone(),
                ResourceEntry {
                    name: id,
                    backend: backend.into(),
                    allowed_roles: None,
                },
            );
    }

    /// 完整注册：展示名 + 角色授权。
    pub fn register_full(
        &self,
        id: impl Into<String>,
        name: impl Into<String>,
        backend: impl Into<String>,
        allowed_roles: Option<Vec<String>>,
    ) {
        self.map
            .write()
            .expect("registry mutex poisoned")
            .insert(
                id.into(),
                ResourceEntry {
                    name: name.into(),
                    backend: backend.into(),
                    allowed_roles,
                },
            );
    }

    /// 列出对外资源信息（id/name，不含 host/port 与角色授权）。
    pub fn list(&self) -> Vec<ResourceInfo> {
        self.map
            .read()
            .expect("registry mutex poisoned")
            .iter()
            .map(|(id, e)| ResourceInfo {
                id: id.clone(),
                name: e.name.clone(),
            })
            .collect()
    }

    /// allowed_roles 判定：None 或空 = 所有角色可用；否则需有交集。
    fn roles_allowed(allowed_roles: Option<&[String]>, roles: &[String]) -> bool {
        match allowed_roles {
            None => true,
            Some(list) if list.is_empty() => true,
            Some(list) => roles.iter().any(|r| list.contains(r)),
        }
    }

    /// 依据 resource_id 解析后端 "host:port" 并做角色级授权检查。
    pub fn resolve_backend(&self, resource_id: &str, roles: &[String]) -> BackendResolution {
        let lookup = self
            .map
            .read()
            .expect("registry mutex poisoned")
            .get(resource_id)
            .cloned();

        if let Some(entry) = lookup {
            if !Self::roles_allowed(entry.allowed_roles.as_deref(), roles) {
                return BackendResolution::Forbidden;
            }
            return if parse_host_port(&entry.backend).is_some() {
                BackendResolution::Allowed(entry.backend)
            } else {
                BackendResolution::NotFound
            };
        }

        // 显式 "host:port" 直连：必须格式合法且主机在私网白名单内。
        let Some((host, port)) = parse_host_port(resource_id) else {
            return BackendResolution::NotFound;
        };
        if !is_allowed_backend(host) {
            return BackendResolution::NotFound;
        }
        BackendResolution::Allowed(format!("{host}:{port}"))
    }
}

/// 解析 "host:port"，返回 (host, port)。
pub fn parse_host_port(addr: &str) -> Option<(&str, u16)> {
    let (host, port) = addr.trim().rsplit_once(':')?;
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    if port == 0 {
        return None;
    }
    Some((host, port))
}

/// SSRF 防护白名单：仅允许私网 IPv4 段与回环（localhost / 127.0.0.0/8 / ::1）。
/// DNS 主机名一律拒绝：网关不代查域名，显式 IP 才允许。
pub fn is_allowed_backend(host: &str) -> bool {
    let host = host.trim();
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if host.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let ip: IpAddr = match host.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) || a == 127
        }
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(id: &str, host: &str, port: u16) -> ResourceConfig {
        ResourceConfig {
            id: id.into(),
            name: id.into(),
            host: host.into(),
            port,
            allowed_roles: None,
        }
    }

    #[test]
    fn allowed_backends_include_private_ranges() {
        for host in [
            "10.0.0.5", "10.255.255.255", "192.168.1.10", "172.16.0.1",
            "172.31.255.254", "localhost", "LOCALHOST", "127.0.0.1",
        ] {
            assert!(is_allowed_backend(host), "{host} 应被允许");
        }
    }

    #[test]
    fn disallowed_backends_reject_public_and_dns() {
        for host in [
            "8.8.8.8", "172.32.0.1", "172.15.0.1", "example.com", "ssh.internal",
            "", "10.0.0.5:22", "0.0.0.0", "169.254.1.1", "2001:db8::1",
        ] {
            assert!(!is_allowed_backend(host), "{host} 应被拒绝");
        }
        assert!(is_allowed_backend("::1"));
    }

    #[test]
    fn parse_host_port_requires_explicit_format() {
        assert_eq!(parse_host_port("10.0.0.5:22"), Some(("10.0.0.5", 22)));
        assert_eq!(parse_host_port(" localhost :2222 "), Some(("localhost", 2222)));
        assert_eq!(parse_host_port("10.0.0.5"), None);
        assert_eq!(parse_host_port("10.0.0.5:ssh"), None);
        assert_eq!(parse_host_port("10.0.0.5:0"), None);
        assert_eq!(parse_host_port(":22"), None);
    }

    #[test]
    fn registry_resolves_configured_resource_and_port_default() {
        let reg = ResourceRegistry::from_configs(&[cfg("ssh-jump01", "10.0.0.5", 22)]);
        assert_eq!(
            reg.resolve_backend("ssh-jump01", &[]),
            BackendResolution::Allowed("10.0.0.5:22".into())
        );
        let c = ResourceConfig {
            id: "custom".into(),
            name: "x".into(),
            host: "192.168.1.20".into(),
            port: 2222,
            allowed_roles: None,
        };
        let reg2 = ResourceRegistry::from_configs(&[c]);
        assert_eq!(
            reg2.resolve_backend("custom", &[]),
            BackendResolution::Allowed("192.168.1.20:2222".into())
        );
    }

    #[test]
    fn list_hides_backend_details() {
        let reg = ResourceRegistry::from_configs(&[ResourceConfig {
            id: "ssh-jump01".into(),
            name: "Linux 跳板机".into(),
            host: "10.0.0.5".into(),
            port: 22,
            allowed_roles: None,
        }]);
        let items = reg.list();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Linux 跳板机");
    }

    #[test]
    fn explicit_host_port_resolved_only_when_allowed() {
        let reg = ResourceRegistry::default();
        assert_eq!(
            reg.resolve_backend("192.168.1.20:22", &[]),
            BackendResolution::Allowed("192.168.1.20:22".into())
        );
        assert_eq!(reg.resolve_backend("8.8.8.8:22", &[]), BackendResolution::NotFound);
        assert_eq!(reg.resolve_backend("evil.example.com:22", &[]), BackendResolution::NotFound);
        assert_eq!(reg.resolve_backend("not-a-addr", &[]), BackendResolution::NotFound);
    }

    #[test]
    fn allowed_roles_matching_role_is_permitted() {
        let mut c = cfg("ops-only", "10.0.0.5", 22);
        c.allowed_roles = Some(vec!["ops".into(), "admin".into()]);
        let reg = ResourceRegistry::from_configs(&[c]);
        assert_eq!(
            reg.resolve_backend("ops-only", &["ops".to_string()]),
            BackendResolution::Allowed("10.0.0.5:22".into())
        );
    }

    #[test]
    fn allowed_roles_mismatch_is_forbidden() {
        let mut c = cfg("ops-only", "10.0.0.5", 22);
        c.allowed_roles = Some(vec!["ops".into(), "admin".into()]);
        let reg = ResourceRegistry::from_configs(&[c]);
        assert_eq!(
            reg.resolve_backend("ops-only", &["contractor".to_string()]),
            BackendResolution::Forbidden,
            "角色无交集 -> Forbidden（不泄露后端地址）"
        );
        assert_eq!(reg.resolve_backend("ops-only", &[]), BackendResolution::Forbidden);
    }

    #[test]
    fn allowed_roles_missing_or_empty_allows_every_role() {
        let reg = ResourceRegistry::from_configs(&[
            cfg("no-roles", "10.0.0.5", 22),
            ResourceConfig {
                id: "empty-roles".into(),
                name: "x".into(),
                host: "10.0.0.6".into(),
                port: 22,
                allowed_roles: Some(vec![]),
            },
        ]);
        for id in ["no-roles", "empty-roles"] {
            for roles in [vec![], vec!["user".to_string()]] {
                assert!(matches!(
                    reg.resolve_backend(id, &roles),
                    BackendResolution::Allowed(_)
                ));
            }
        }
    }
}
