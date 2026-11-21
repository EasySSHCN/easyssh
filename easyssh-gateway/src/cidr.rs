//! dev_mode 来源 IP 白名单：CIDR 解析与匹配（R-20261120-006 闸门组成之二）。
//!
//! 支持 IPv4 `a.b.c.d/p` 与 IPv6 `x::y/p`；不带 `/p` 的裸地址按全长度
//! （/32 或 /128）处理。解析失败返回 None（调用方按 fail-fast 处理，
//! 避免静默放行）。空白名单默认只放行回环（127.0.0.0/8 + ::1/128）。

/// 单条 CIDR：网络地址 + 前缀长度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cidr {
    pub addr: std::net::IpAddr,
    pub bits: u8,
}

impl Cidr {
    /// 解析 "1.2.3.0/24" 或 "10.0.0.5"（裸地址 = /32）。
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let (addr_part, bits) = match s.rsplit_once('/') {
            Some((a, p)) => {
                let bits = p.trim().parse::<u8>().ok()?;
                (a, bits)
            }
            None => (s, 0), // bits 按地址族补全
        };
        let addr: std::net::IpAddr = addr_part.trim().parse().ok()?;
        let max = match addr {
            std::net::IpAddr::V4(_) => 32u8,
            std::net::IpAddr::V6(_) => 128u8,
        };
        let bits = if bits == 0 { max } else if bits > max { return None } else { bits };
        Some(Self { addr, bits })
    }

    /// 判断 IP 是否落在本网段。
    pub fn matches(&self, ip: std::net::IpAddr) -> bool {
        match (self.addr, ip) {
            (std::net::IpAddr::V4(net), std::net::IpAddr::V4(ip)) => {
                prefix_eq(u32::from(net), u32::from(ip), self.bits)
            }
            (std::net::IpAddr::V6(net), std::net::IpAddr::V6(ip)) => {
                prefix_eq128(net, ip, self.bits)
            }
            _ => false, // 地址族不匹配
        }
    }
}

fn prefix_eq(net: u32, ip: u32, bits: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let mask = if bits >= 32 { u32::MAX } else { !((1u32 << (32 - bits)) - 1) };
    (net & mask) == (ip & mask)
}

fn prefix_eq128(net: std::net::Ipv6Addr, ip: std::net::Ipv6Addr, bits: u8) -> bool {
    let n = u128::from(net);
    let i = u128::from(ip);
    if bits == 0 {
        return true;
    }
    let mask = if bits >= 128 { u128::MAX } else { !((1u128 << (128 - bits)) - 1) };
    (n & mask) == (i & mask)
}

/// 缺省白名单：仅回环（dev_mode_allow_cidrs 未配置时的隐式值）。
pub fn loopback_defaults() -> Vec<Cidr> {
    vec![
        Cidr { addr: std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 0)), bits: 8 },
        Cidr { addr: std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST), bits: 128 },
    ]
}

/// 解析配置中的 CIDR 列表；任一条非法即整体失败（fail-fast，防静默放行）。
pub fn parse_cidrs(items: &[String]) -> Result<Vec<Cidr>, String> {
    if items.is_empty() {
        return Ok(loopback_defaults());
    }
    items
        .iter()
        .map(|s| Cidr::parse(s).ok_or_else(|| format!("非法 CIDR: {s:?}")))
        .collect()
}

/// 来源 IP 字符串是否在白名单内。"unknown"（无 ConnectInfo / 反代未透传）
/// 一律视为不在白名单内（fail-closed）。
pub fn ip_allowed(ip: &str, cidrs: &[Cidr]) -> bool {
    let Ok(addr) = ip.trim().parse::<std::net::IpAddr>() else {
        return false;
    };
    cidrs.iter().any(|c| c.matches(addr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Option<Cidr> {
        Cidr::parse(s)
    }

    #[test]
    fn parse_ipv4_prefixes() {
        let c = v("10.0.0.0/8").unwrap();
        assert_eq!(c.bits, 8);
        assert!(c.matches("10.1.2.3".parse().unwrap()));
        assert!(!c.matches("11.0.0.1".parse().unwrap()));
        let c = v("172.16.0.0/12").unwrap();
        assert!(c.matches("172.31.255.254".parse().unwrap()));
        assert!(!c.matches("172.32.0.1".parse().unwrap()));
    }

    #[test]
    fn parse_bare_address_is_full_prefix() {
        let c = v("192.168.1.5").unwrap();
        assert_eq!(c.bits, 32);
        assert!(c.matches("192.168.1.5".parse().unwrap()));
        assert!(!c.matches("192.168.1.6".parse().unwrap()));
        let c = v("::1").unwrap();
        assert_eq!(c.bits, 128);
        assert!(c.matches("::1".parse().unwrap()));
    }

    #[test]
    fn parse_invalid_rejected() {
        assert!(v("").is_none());
        assert!(v("  ").is_none());
        assert!(v("10.0.0.0/33").is_none());
        assert!(v("10.0.0.0/notaprefix").is_none());
        assert!(v("not-an-ip/8").is_none());
        assert!(v("::1/129").is_none());
    }

    #[test]
    fn parse_cidrs_fail_fast_on_bad_entry() {
        assert!(parse_cidrs(&["10.0.0.0/8".into(), "bad".into()]).is_err());
        assert_eq!(parse_cidrs(&[]).unwrap(), loopback_defaults());
        assert_eq!(parse_cidrs(&["127.0.0.1".into()]).unwrap().len(), 1);
    }

    #[test]
    fn ip_allowed_fail_closed_on_unknown() {
        let cidrs = loopback_defaults();
        assert!(ip_allowed("127.0.0.1", &cidrs));
        assert!(ip_allowed("127.9.9.9", &cidrs));
        assert!(ip_allowed("::1", &cidrs));
        assert!(!ip_allowed("unknown", &cidrs), "无来源信息必须 fail-closed");
        assert!(!ip_allowed("", &cidrs));
        assert!(!ip_allowed("8.8.8.8", &cidrs));
    }

    #[test]
    fn ipv6_prefix_match() {
        let c = v("2001:db8::/32").unwrap();
        assert!(c.matches("2001:db8::1".parse().unwrap()));
        assert!(!c.matches("2001:db9::1".parse().unwrap()));
    }
}
