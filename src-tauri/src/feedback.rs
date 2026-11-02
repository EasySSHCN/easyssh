/* EasySSH v4：使用反馈加密提交（对应 3.x feedback.js）
 * 信封 = { v:1, alg:"A256GCM-HS256", ts:<ms>, nonce:b64(12B), ct:b64, tag:b64, mac:b64 }
 * ct  = AES-256-GCM(fbEncKey, nonce, 明文JSON)
 * mac = HMAC-SHA256(fbMacKey, `${ts}.${nonce}.${ct}.${tag}`)
 * fbEncKey = HMAC-SHA256(master,"enc")、fbMacKey = HMAC-SHA256(master,"mac")
 * （与官网 api/feedback/common.php 完全一致；主密钥复用 update-keys.json） */
use std::path::PathBuf;

use serde_json::{json, Value};
use sha2::Digest;

const API_URL: &str = "https://easyssh.cn/api/feedback/submit.php";

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("base64 解码失败: {e}"))
}
fn b64e(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::Mac;
    let mut hm = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac key");
    hm.update(data);
    hm.finalize().into_bytes().to_vec()
}

pub async fn submit(
    base_dir: &PathBuf,
    mut payload: Value,
) -> Result<Value, String> {
    // v4:提交信息中隐藏携带软件端版本号(渲染端未传/传空时自动补齐,与官网反馈库 appVersion 字段对接)
    let ver = env!("CARGO_PKG_VERSION");
    let cur = payload
        .get("appVersion")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if cur.is_empty() || cur != ver {
        payload
            .as_object_mut()
            .ok_or("payload 必须是对象")?
            .insert("appVersion".into(), json!(ver));
    }
    // systemInfo:官网反馈库/巡检邮件的「计算机/网络信息」数据源
    // v4.1.3:对齐 3.x 采集范围并扩展——CPU/内存/硬盘/主板/显卡/网卡(MAC/IP)/公网IP 等
    if payload.get("systemInfo").map(|x| !x.is_object()).unwrap_or(true) {
        payload
            .as_object_mut()
            .ok_or("payload 必须是对象")?
            .insert("systemInfo".into(), collect_system_info(ver));
    }
    // 主密钥（复用 update-keys.json）
    // 密钥来源与升级一致：exe 同目录文件优先，否则构建期嵌入密钥
    let master = crate::updater::load_master(base_dir)?;

    let enc_key = hmac_sha256(&master, b"enc");
    let mac_key = hmac_sha256(&master, b"mac");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut nonce = [0u8; 12];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut nonce);
    let nonce_s = b64e(&nonce);

    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let a = aes_gcm::Aes256Gcm::new_from_slice(&enc_key).map_err(|e| e.to_string())?;
    let full = a
        .encrypt(
            aes_gcm::Nonce::from_slice(&nonce),
            Payload::from(serde_json::to_string(&payload).map_err(|e| e.to_string())?.as_bytes()),
        )
        .map_err(|e| e.to_string())?;
    let (ct, tag) = full.split_at(full.len() - 16);
    let (ct_s, tag_s) = (b64e(ct), b64e(tag));
    let mac = {
        let mac_data = format!("{ts}.{nonce_s}.{ct_s}.{tag_s}");
        b64e(&hmac_sha256(&mac_key, mac_data.as_bytes()))
    };

    let envelope = json!({
        "v": 1, "alg": "A256GCM-HS256", "ts": ts,
        "nonce": nonce_s, "ct": ct_s, "tag": tag_s, "mac": mac,
    });

    // POST（阻塞客户端放线程池）
    tokio::task::spawn_blocking(move || {
        let resp = ureq::post(API_URL)
            .timeout(std::time::Duration::from_secs(15))
            .set("Content-Type", "application/json")
            .send_string(&envelope.to_string())
            .map_err(|e| match e {
                ureq::Error::Status(code, r) => {
                    let msg = r.into_string().unwrap_or_default();
                    if let Ok(j) = serde_json::from_str::<Value>(&msg) {
                        if let Some(m) = j.get("message").and_then(|x| x.as_str()) {
                            return format!("提交失败: {m}");
                        }
                    }
                    format!("提交失败: HTTP {code}")
                }
                other => format!("提交失败: {other}"),
            })?;
        let body = resp.into_string().map_err(|e| format!("读取响应失败: {e}"))?;
        let j: Value = serde_json::from_str(&body).map_err(|e| format!("响应解析失败: {e}"))?;
        Ok(j)
    })
    .await
    .map_err(|e| format!("任务错误: {e}"))?
}

// 抑制未使用（Digest 为 SHA-256 预留；当前未直接使用）
#[allow(dead_code)]
fn _d(b: &[u8]) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(b).to_vec()
}




// ================= v4.1.6：纯 WinAPI 原生采集（彻底替代 PowerShell 子进程） =================

fn hostname_str() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::fs::read_to_string("/proc/sys/kernel/hostname").map(|s| s.trim().to_string()))
        .unwrap_or_else(|_| "unknown".into())
}

fn cap_str(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn insert_capped(map: &mut serde_json::Map<String, serde_json::Value>, k: &str, v: String) {
    if v.trim().is_empty() { return; }
    if map.len() >= 24 { return; }
    map.insert(cap_str(k, 24), json!(cap_str(&v, 300)));
}
// 360 等安全软件会拦截 PowerShell 命令执行（弹窗干扰用户），改用注册表 + 系统 API 直读，
// 全程无子进程、无脚本执行，静默完成；任一项失败自动跳过。

#[cfg(target_os = "windows")]
mod winffi {
    pub fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    pub fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    pub const REG_SZ: u32 = 1;
    pub const REG_DWORD: u32 = 4;
    pub const RRF_RT_REG_SZ: u32 = 0x2;
    pub const RRF_RT_REG_DWORD: u32 = 0x4;

    #[link(name = "advapi32")]
    extern "system" {
        pub fn RegGetValueW(
            hkey: isize,
            lpsubkey: *const u16,
            lpvalue: *const u16,
            dwflags: u32,
            pdwtype: *mut u32,
            pvdata: *mut u8,
            pcbdata: *mut u32,
        ) -> i32;
    }

    pub const HKEY_LOCAL_MACHINE: isize = 0x8000_0002;

    /// 读注册表字符串值（RegGetValueW 自动开合句柄）
    pub fn reg_sz(subkey: &str, value: &str) -> Option<String> {
        let sk = to_wide_null(subkey);
        let vn = to_wide_null(value);
        let mut buf = [0u8; 1024];
        let mut size = buf.len() as u32;
        let mut ty = 0u32;
        let rc = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, sk.as_ptr(), vn.as_ptr(), RRF_RT_REG_SZ, &mut ty, buf.as_mut_ptr(), &mut size)
        };
        if rc != 0 { return None; }
        let u16s: Vec<u16> = buf[..size as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let s = from_wide(&u16s);
        let s = s.trim_end_matches('\0').trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetComputerNameW(lpbuffer: *mut u16, nsize: *mut u32) -> i32;
        pub fn GetTickCount64() -> u64;
        pub fn GetUserDefaultUILanguage() -> u16;
        pub fn GlobalMemoryStatusEx(lpbuffer: *mut MEMORYSTATUSEX) -> i32;
        pub fn GetLogicalDriveStringsW(nbufferlength: u32, lpbuffer: *mut u16) -> u32;
        pub fn GetDriveTypeW(lprootpathname: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(
            lpdirectoryname: *const u16,
            lpfreebytesavailabletocaller: *mut u64,
            lptotalnumberofbytes: *mut u64,
            lptotalnumberoffreebytes: *mut u64,
        ) -> i32;
        pub fn GetSystemInfo(lpsysteminfo: *mut SYSTEM_INFO);
    }

    #[repr(C)]
    pub struct MEMORYSTATUSEX {
        pub dw_length: u32,
        pub dw_memory_load: u32,
        pub ull_total_phys: u64,
        pub ull_avail_phys: u64,
        pub ull_total_pagefile: u64,
        pub ull_avail_pagefile: u64,
        pub ull_total_virtual: u64,
        pub ull_avail_virtual: u64,
        pub ull_avail_extended_virtual: u64,
    }

    #[repr(C)]
    pub struct SYSTEM_INFO {
        pub w_processor_architecture: u16,
        pub w_reserved: u16,
        pub dw_page_size: u32,
        pub lp_min_addr: *mut u8,
        pub lp_max_addr: *mut u8,
        pub dw_active_mask: usize,
        pub dw_number_of_processors: u32,
        pub dw_processor_type: u32,
        pub dw_alloc_granularity: u32,
        pub w_processor_level: u16,
        pub w_processor_revision: u16,
    }

    pub fn computer_name() -> Option<String> {
        let mut buf = [0u16; 128];
        let mut n = buf.len() as u32;
        if unsafe { GetComputerNameW(buf.as_mut_ptr(), &mut n) } != 0 {
            Some(from_wide(&buf[..n as usize]))
        } else {
            None
        }
    }

    pub fn uptime_hours() -> Option<u64> {
        Some(unsafe { GetTickCount64() } / 3_600_000)
    }

    pub fn ui_language() -> Option<String> {
        Some(format!("{}", unsafe { GetUserDefaultUILanguage() }))
    }

    pub fn total_mem_gb() -> Option<String> {
        let mut ms = MEMORYSTATUSEX {
            dw_length: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            dw_memory_load: 0, ull_total_phys: 0, ull_avail_phys: 0,
            ull_total_pagefile: 0, ull_avail_pagefile: 0,
            ull_total_virtual: 0, ull_avail_virtual: 0, ull_avail_extended_virtual: 0,
        };
        if unsafe { GlobalMemoryStatusEx(&mut ms) } != 0 {
            Some(format!("{:.1}GB", ms.ull_total_phys as f64 / 1073741824.0))
        } else {
            None
        }
    }

    pub fn logical_cpus() -> Option<u32> {
        let mut si = SYSTEM_INFO {
            w_processor_architecture: 0, w_reserved: 0, dw_page_size: 0,
            lp_min_addr: std::ptr::null_mut(), lp_max_addr: std::ptr::null_mut(),
            dw_active_mask: 0, dw_number_of_processors: 0, dw_processor_type: 0,
            dw_alloc_granularity: 0, w_processor_level: 0, w_processor_revision: 0,
        };
        unsafe { GetSystemInfo(&mut si) };
        Some(si.dw_number_of_processors)
    }

    pub fn disks() -> Vec<String> {
        let mut buf = [0u16; 512];
        let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) };
        if n == 0 || n as usize > buf.len() { return Vec::new(); }
        let mut out = Vec::new();
        for root in from_wide(&buf[..n as usize]).split('\0').filter(|s| !s.is_empty()) {
            let ty = unsafe { GetDriveTypeW(to_wide_null(root).as_ptr()) };
            if ty != 3 { continue; } // 3 = DRIVE_FIXED
            let root_w = to_wide_null(root);
            let mut free = 0u64;
            let mut total = 0u64;
            let mut dummy = 0u64;
            if unsafe { GetDiskFreeSpaceExW(root_w.as_ptr(), &mut free, &mut total, &mut dummy) } != 0 && total > 0 {
                out.push(format!("{} {}GB(剩{}GB)", root.trim_end_matches('\\'),
                    total / 1_000_000_000, free / 1_000_000_000));
            }
        }
        out
    }

    // ---- OS 版本（RtlGetVersion，兼容性视图不受清单限制） ----
    #[repr(C)]
    pub struct OsVersionInfoW {
        pub size: u32,
        pub major: u32,
        pub minor: u32,
        pub build: u32,
        pub platform_id: u32,
        pub csd: [u16; 128],
    }

    #[link(name = "ntdll")]
    extern "system" {
        pub fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }

    pub fn os_version() -> Option<(u32, u32, u32)> {
        let mut v = OsVersionInfoW {
            size: std::mem::size_of::<OsVersionInfoW>() as u32,
            major: 0, minor: 0, build: 0, platform_id: 0, csd: [0; 128],
        };
        if unsafe { RtlGetVersion(&mut v) } == 0 {
            Some((v.major, v.minor, v.build))
        } else {
            None
        }
    }

    // ---- 网卡（GetAdaptersAddresses，iphlpapi 运行时绑定，免导入库） ----
    pub const AF_INET: u16 = 2;

    pub fn adapters_v4() -> Vec<String> {
        unsafe extern "system" {
            fn LoadLibraryW(name: *const u16) -> isize;
            fn GetProcAddress(module: isize, name: *const u8) -> isize;
        }
        let name = to_wide_null("iphlpapi.dll\0");
        let h = unsafe { LoadLibraryW(name.as_ptr()) };
        if h == 0 { return Vec::new(); }
        let proc_name = b"GetAdaptersAddresses\0";
        let gaa = unsafe { GetProcAddress(h, proc_name.as_ptr()) };
        if gaa == 0 { return Vec::new(); }
        type GetAdaptersAddressesFn =
            unsafe extern "system" fn(u16, u64, *mut u8, *mut u8, *mut u32) -> i32;
        let gaa: GetAdaptersAddressesFn = unsafe { std::mem::transmute(gaa) };

        let mut size = 0u32;
        let flags: u64 = 0;
        unsafe {
            let _ = gaa(AF_INET, flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut size);
        }
        if size == 0 { return Vec::new(); }
        let mut buf = vec![0u8; (size as usize) + 64];
        let rc = unsafe { gaa(AF_INET, flags, std::ptr::null_mut(), buf.as_mut_ptr(), &mut size) };
        if rc != 0 { return Vec::new(); }

        let mut out = Vec::new();
        let mut cur = buf.as_mut_ptr() as usize;
        // IP_ADAPTER_ADDRESSES_LH x64 字段偏移：
        // +0 Length(u32) +4 IfIndex(u32) | +8 Next | +16 AdapterName
        // | +24 FirstUnicastAddress | +56 DnsSuffix | +64 Description | +72 FriendlyName
        // | +80 PhysicalAddress[8] | +88 PhysicalAddressLength | +100 IfType
        while cur != 0 {
            let if_type = unsafe { (cur as *const u32).add(25).read_unaligned() }; // +100
            let friendly_ptr = unsafe { (cur as *const *const u16).byte_add(72).read() };
            let mac_len = unsafe { (cur as *const u32).byte_add(88).read_unaligned() };
            let friendly = if friendly_ptr.is_null() {
                String::new()
            } else {
                let mut end = 0usize;
                unsafe {
                    while *(friendly_ptr.add(end)) != 0 && end < 256 { end += 1; }
                }
                from_wide(unsafe { std::slice::from_raw_parts(friendly_ptr, end) })
            };
            let mac = if mac_len > 0 && mac_len <= 8 {
                let bytes = unsafe { std::slice::from_raw_parts((cur + 80) as *const u8, mac_len as usize) };
                bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join("-")
            } else {
                String::new()
            };
            // 单播地址链：+24 FirstUnicastAddress → IP_ADAPTER_UNICAST_ADDRESS:
            // +8 Next | +16 sockaddr(SOCKET_ADDRESS{lpSockaddr,iLen}) → sa_family@0, addr@4(v4)/@8(v6)
            let mut ua = unsafe { (cur as *const usize).byte_add(24).read() };
            let mut shown = 0;
            while ua != 0 && shown < 2 {
                let sa = unsafe { (ua as *const usize).byte_add(16).read() };
                if sa != 0 {
                    let family = unsafe { (sa as *const u16).read_unaligned() };
                    if family == AF_INET {
                        let a = unsafe { std::slice::from_raw_parts((sa + 4) as *const u8, 4) };
                        let ip = format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]);
                        if !ip.starts_with("169.254.") {
                            out.push(format!("{}[{} {}/v4]", friendly, mac, ip));
                            shown += 1;
                        }
                    }
                }
                ua = unsafe { (ua as *const usize).byte_add(8).read() };
            }
            cur = unsafe { (cur as *const usize).byte_add(8).read() };
            // IfType 24 = loopback, 131 = tunnel：跳过
            if (if_type == 24 || if_type == 131) && !out.is_empty() {
                out.pop();
            }
        }
        out
    }
}



// ================= v4.1.8 网卡采集重写（标准结构体 + u32 Family）+ 公网 IP 解析加固 =================

#[cfg(target_os = "windows")]
pub mod netcollect {
    // 与 Windows SDK 头文件一致的 x64 结构体定义
    #[repr(C)]
    pub struct SockaddrIn {
        pub sin_family: u16,
        pub sin_port: u16,
        pub sin_addr: [u8; 4],
        pub sin_zero: [u8; 8],
    }

    #[repr(C)]
    pub struct SocketAddress {
        pub lp_sockaddr: *mut SockaddrIn,
        pub i_sockaddr_len: i32,
    }

    #[repr(C)]
    pub struct AdapterUnicastAddressLh {
        pub alignment: u64,
        pub next: *mut AdapterUnicastAddressLh,
        pub address: SocketAddress,
        pub dk_context: *mut u8,
        pub cache_time: u64,
        pub timestamp: u64,
    }

    #[repr(C)]
    pub struct AdapterAddressesLh {
        pub hdr: u64, // 低 32 位 Length，高 32 位 IfIndex
        pub next: *mut AdapterAddressesLh,
        pub adapter_name: *mut u8,
        pub first_unicast: *mut AdapterUnicastAddressLh,
        pub first_anycast: *mut u8,
        pub first_multicast: *mut u8,
        pub first_dns_server: *mut u8,
        pub dns_suffix: *mut u16,
        pub description: *mut u16,
        pub friendly_name: *mut u16,
        pub physical_address: [u8; 8],
        pub physical_address_length: u32,
        pub flags: u32,
        pub mtu: u32,
        pub if_type: u32,
        pub oper_status: u32,
    }

    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> isize;
    }

    type GetAdaptersAddressesFn =
        unsafe extern "system" fn(family: u32, flags: u32, reserved: *mut u8, adapters: *mut AdapterAddressesLh, size: *mut u32) -> i32;

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read_wide(ptr: *const u16) -> String {
        if ptr.is_null() { return String::new(); }
        let mut end = 0usize;
        unsafe {
            while *ptr.add(end) != 0 && end < 512 { end += 1; }
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, end) })
    }

    /// 采集网卡：名称[MAC IPv4]（跳过回环/隧道/链路本地地址；无 v4 地址的网卡只列 MAC）
    pub fn collect() -> Vec<String> {
        let name = wide("iphlpapi.dll\0");
        let h = unsafe { LoadLibraryW(name.as_ptr()) };
        if h == 0 { return Vec::new(); }
        let pname = b"GetAdaptersAddresses\0";
        let gaa_raw = unsafe { GetProcAddress(h, pname.as_ptr()) };
        if gaa_raw == 0 { return Vec::new(); }
        let gaa: GetAdaptersAddressesFn = unsafe { std::mem::transmute(gaa_raw) };

        const AF_INET: u16 = 2;
        let mut size = 0u32;
        unsafe { gaa(AF_INET as u32, 0, std::ptr::null_mut(), std::ptr::null_mut(), &mut size) };
        if size < std::mem::size_of::<AdapterAddressesLh>() as u32 { return Vec::new(); }
        let mut buf = vec![0u8; size as usize + 128];
        let rc = unsafe {
            gaa(AF_INET as u32, 0, std::ptr::null_mut(), buf.as_mut_ptr() as *mut AdapterAddressesLh, &mut size)
        };
        if rc != 0 { return Vec::new(); }

        let mut out = Vec::new();
        let mut cur = buf.as_ptr() as *const AdapterAddressesLh;
        let mut guard = 0;
        while !cur.is_null() && guard < 64 {
            guard += 1;
            let ad = unsafe { &*cur };
            let friendly = read_wide(ad.friendly_name);
            let mac = if ad.physical_address_length > 0 {
                ad.physical_address[..ad.physical_address_length as usize]
                    .iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join("-")
            } else { String::new() };
            // IfType: 6=以太网 71=WLAN 233=虚拟；24=回环 131=隧道 跳过
            let interesting_type = ad.if_type == 6 || ad.if_type == 71 || ad.if_type == 233;
            let mut entry = format!("{}[{}", friendly, mac);
            let mut ip = None;
            let mut ua = ad.first_unicast;
            let mut guard2 = 0;
            while !ua.is_null() && guard2 < 16 {
                guard2 += 1;
                let u = unsafe { &*ua };
                let sa = u.address.lp_sockaddr;
                if !sa.is_null() {
                    let family = unsafe { (*sa).sin_family };
                    if family == AF_INET {
                        let a = unsafe { (*sa).sin_addr };
                        let s = format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]);
                        if !s.starts_with("169.254.") && !s.starts_with("127.") {
                            ip = Some(s);
                            break;
                        }
                    }
                }
                ua = u.next;
            }
            if let Some(ref v) = ip { entry.push(' '); entry.push_str(&v); entry.push_str("/v4"); }
            entry.push(']');
            if interesting_type || ip.is_some() {
                out.push(entry);
            }
            cur = ad.next;
        }
        out
    }
}

#[cfg(target_os = "windows")]
fn parse_public_ip_response(t: &str) -> Option<String> {
    let mut s = t.trim().trim_matches('"').to_string();
    // 某些接口返回 JSON（如 {"ip":"1.2.3.4"}），提取其中的 ip 字段值
    if s.starts_with('{') {
        if let Some(p) = s.split("\"ip\":\"").nth(1) {
            if let Some(v) = p.split('"').next() { s = v.to_string(); }
        }
    }
    let s = s.trim().trim_matches('"').to_string();
    // IPv4：4 段 0-255
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
        return Some(s);
    }
    // IPv6：含冒号的十六进制串
    if s.contains(':') && s.len() >= 2 && s.len() <= 45
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
        return Some(s);
    }
    None
}

#[cfg(target_os = "windows")]
fn fetch_public_ip() -> Option<String> {
    // v4.1.5:多端点冗余——主接口 ip.263.tw，失败依次降级（v4.1.8:增加响应解析，兼容 JSON 包装）
    const ENDPOINTS: [&str; 5] = [
        "https://ip.263.tw/ip",      // 主接口
        "https://api.ipify.org",     // 备用 1
        "https://ifconfig.me/ip",    // 备用 2
        "https://icanhazip.com",     // 备用 3
        "https://ipecho.net/plain",  // 备用 4
    ];
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    for url in ENDPOINTS {
        let txt = match agent.get(url).call() {
            Ok(resp) => match resp.into_string() {
                Ok(t) => t,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        if let Some(ip) = parse_public_ip_response(&txt) {
            return Some(ip);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn collect_windows_info(map: &mut serde_json::Map<String, serde_json::Value>) {
    use winffi;

    // 操作系统：产品名 + 显示版本 + Build（RtlGetVersion 不受应用清单影响，永远返回真实版本）
    let ver = winffi::os_version();
    let product = winffi::reg_sz(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName");
    let display = winffi::reg_sz(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "DisplayVersion");
    if let Some((ma, mi, build)) = ver {
        let mut s = product.clone().unwrap_or_else(|| "Windows".into());
        if let Some(d) = &display { s = format!("{} {}", s, d); }
        s = format!("{} (Build {}.{})", s, build, format!("{}.{}", ma, mi));
        insert_capped(map, "操作系统", s);
    }
    if let Some(a) = std::env::var("PROCESSOR_ARCHITECTURE").ok() {
        insert_capped(map, "程序架构", a);
    }
    // CPU：注册表型号 + 逻辑处理器数
    let cpu_name = winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString");
    if let Some(n) = cpu_name.clone() {
        let threads = winffi::logical_cpus().unwrap_or(0);
        insert_capped(map, "CPU", format!("{} / {}逻辑处理器", n.trim(), threads));
    }
    if let Some(m) = winffi::total_mem_gb() {
        insert_capped(map, "内存", m);
    }
    // 主板/BIOS：注册表 BIOS 键
    let board_mfr = winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardManufacturer");
    let board_prod = winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardProduct");
    if board_mfr.is_some() || board_prod.is_some() {
        insert_capped(map, "主板", format!("{} {}", board_mfr.unwrap_or_default(), board_prod.unwrap_or_default()));
    }
    if let Some(b) = winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\BIOS", "BIOSVersion") {
        insert_capped(map, "BIOS", b);
    }
    if let Some(h) = winffi::uptime_hours() {
        insert_capped(map, "系统运行时长", format!("{}小时", h));
    }
    if let Some(l) = winffi::ui_language() {
        insert_capped(map, "系统语言代码", l);
    }
    // 显卡：注册表显示适配器类枚举（0000..0009），去重
    let mut gpus = Vec::new();
    for i in 0..10 {
        if let Some(desc) = winffi::reg_sz(
            &format!(r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{:04}", i),
            "DriverDesc") {
            if !gpus.contains(&desc) { gpus.push(desc); }
        }
    }
    if !gpus.is_empty() { insert_capped(map, "显卡", gpus.join(" / ")); }
    // 磁盘
    let disks = winffi::disks();
    if !disks.is_empty() { insert_capped(map, "磁盘", disks.join(", ")); }
    // 网卡（MAC/内网 IPv4）
    let nics = netcollect::collect();
    if !nics.is_empty() { insert_capped(map, "网卡(MAC/IP)", nics.join(", ")); }
    // 公网出口 IP
    if let Some(ip) = fetch_public_ip() {
        insert_capped(map, "公网IP", ip);
    }
}

#[cfg(not(target_os = "windows"))]
fn collect_windows_info(_map: &mut serde_json::Map<String, serde_json::Value>) {}

fn collect_system_info(version: &str) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    insert_capped(&mut map, "软件版本", version.to_string());
    insert_capped(&mut map, "主机名", hostname_str());
    insert_capped(&mut map, "系统平台", format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH));
    collect_windows_info(&mut map);
    serde_json::Value::Object(map)
}
