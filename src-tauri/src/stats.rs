/* EasySSH v5.1.0：客户端匿名使用情况统计
 *
 * 用途边界（合规）：
 *  - 仅收集设备环境概要用于纯统计（操作系统/CPU/内存档/磁盘档/语言/地域/版本/活跃度）；
 *  - 设备标识为 SHA-256 去标识化匿名ID（主机名+主板型号+固定盐派生），不可反推原值；
 *  - 不上报：主机名原文、网卡 MAC、内网 IP、磁盘分区明细；公网 IP 仅保留前两段；
 *  - 默认开启，可在「设置 → 升级 → 隐私」关闭；关闭后不再上报；
 *  - 加密通道与反馈同族（AES-256-GCM + HMAC-SHA256 时效签名），
 *    服务端 api/stats/report.php 白名单过滤 + 同设备 12h 去抖。
 *
 * 上报节奏：每天首次启动（≥20h 间隔）延迟 90 秒后台执行，失败静默（下次启动重试），
 * 不影响启动速度，无任何 UI 弹窗。
 */
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::Digest;

const API_URL: &str = "https://easyssh.cn/api/stats/report.php";
const LAST_FILE: &str = "stats-last.json"; // exe 同目录：{ "lastMs": ... }
const MIN_INTERVAL_MS: u64 = 20 * 3600 * 1000; // 每天一次（20h 去抖）
const ANON_SALT: &str = "easyssh-stats-anon-v1";

/* ---------------- 匿名设备ID ---------------- */

#[cfg(target_os = "windows")]
fn board_identity() -> String {
    use crate::feedback::winffi;
    winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardProduct")
        .or_else(|| winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardManufacturer"))
        .unwrap_or_default()
}

#[cfg(not(target_os = "windows"))]
fn board_identity() -> String {
    String::new()
}

/// SHA-256(主机名 + 主板型号 + 固定盐) 前 16 个 hex —— 去标识化，不可反推
pub(crate) fn anon_id() -> String {
    let raw = format!(
        "{}|{}|{}",
        crate::feedback::hostname_str(),
        board_identity(),
        ANON_SALT
    );
    hex16(&sha2::Sha256::digest(raw.as_bytes()))
}

fn hex16(d: &[u8]) -> String {
    d.iter().take(8).map(|x| format!("{x:02x}")).collect()
}

/* ---------------- 采集（仅统计字段，白名单见服务端） ---------------- */

#[cfg(target_os = "windows")]
fn collect_fields(version: &str) -> serde_json::Map<String, Value> {
    use crate::feedback::{fetch_public_ip, winffi};
    let mut m = serde_json::Map::new();
    let mut put = |k: &str, v: String| {
        let v = v.trim().to_string();
        if !v.is_empty() {
            m.insert(k.to_string(), json!(v.chars().take(160).collect::<String>()));
        }
    };

    put("客户端类型", "easyssh-windows".into());
    put("软件版本", version.to_string());
    // 操作系统：产品名 + DisplayVersion（不含 Build 明细，降低指纹粒度）
    let product = winffi::reg_sz(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName")
        .unwrap_or_else(|| "Windows".into());
    let display = winffi::reg_sz(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "DisplayVersion")
        .unwrap_or_default();
    put("操作系统", if display.is_empty() { product } else { format!("{product} {display}") });
    put("系统架构", std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_else(|_| std::env::consts::ARCH.into()));
    if let Some(n) = winffi::reg_sz(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString") {
        // CPU 型号截断到家族级别（去掉频率后缀），进一步降低指纹粒度
        let fam = n.split('@').next().unwrap_or(&n).trim().to_string();
        put("CPU", fam);
    }
    if let Some(gb) = winffi::total_mem_gb() {
        put("内存", mem_bucket(&gb));
    }
    let disks = winffi::disks();
    if !disks.is_empty() {
        // 只取系统盘容量档位（第一块固定磁盘），不传分区明细
        let total_gb = disks[0]
            .split(&[' ', '('][..])
            .filter_map(|t| t.strip_suffix("GB").and_then(|n| n.parse::<u64>().ok()))
            .next()
            .unwrap_or(0);
        put("磁盘容量", disk_bucket(total_gb));
    }
    // 显卡只传首块
    for i in 0..10 {
        if let Some(desc) = winffi::reg_sz(
            &format!(r"SYSTEM\CurrentControlSet\Control\Class\{{4d36e968-e325-11ce-bfc1-08002be10318}}\{:04}", i),
            "DriverDesc") {
            put("显卡", desc);
            break;
        }
    }
    if let Some(l) = winffi::ui_language() {
        put("系统语言代码", l);
    }
    // 公网出口：只保留前两段（合规去标识化），失败则跳过
    if let Some(ip) = fetch_public_ip() {
        let seg: Vec<&str> = ip.split('.').collect();
        if seg.len() == 4 {
            put("地域", format!("{}.{}.x.x", seg[0], seg[1]));
        }
    }
    m
}

#[cfg(not(target_os = "windows"))]
fn collect_fields(version: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("客户端类型".into(), json!("easyssh-other"));
    m.insert("软件版本".into(), json!(version));
    m.insert(
        "操作系统".into(),
        json!(format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)),
    );
    m
}

/// 内存归档位：8GB 以下 / 8-16 / 16-32 / 32GB 以上
fn mem_bucket(s: &str) -> String {
    let gb: f64 = s.trim_end_matches("GB").trim().parse().unwrap_or(0.0);
    if gb <= 0.0 { return String::new(); }
    if gb < 8.0 { "8GB以下".into() }
    else if gb < 16.0 { "8-16GB".into() }
    else if gb < 32.0 { "16-32GB".into() }
    else { "32GB以上".into() }
}

/// 磁盘容量归档位
fn disk_bucket(total_gb: u64) -> String {
    if total_gb == 0 { return String::new(); }
    match total_gb {
        0..=256 => "≤256GB".into(),
        257..=512 => "256-512GB".into(),
        513..=1024 => "512GB-1TB".into(),
        1025..=2048 => "1-2TB".into(),
        _ => "2TB以上".into(),
    }
}

/* ---------------- 去抖状态（exe 同目录 stats-last.json） ---------------- */

fn last_report_ms(base: &Path) -> u64 {
    std::fs::read_to_string(base.join(LAST_FILE))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|j| j.get("lastMs").and_then(|x| x.as_u64()))
        .unwrap_or(0)
}

fn write_last_report(base: &Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let _ = std::fs::write(base.join(LAST_FILE), json!({ "lastMs": now }).to_string());
}

/// 读取设置中的统计开关（缺字段 = 开启）
fn stats_enabled(base: &Path) -> bool {
    easyssh_core::config::ConfigStore::new(base.to_path_buf())
        .load()
        .ok()
        .and_then(|c| c.settings.get("statsEnabled").and_then(|x| x.as_bool()))
        .unwrap_or(true)
}

/* ---------------- 上报入口 ---------------- */

/// 由 main.rs setup 延迟调用：读取开关与去抖状态，符合条件则上报
pub(crate) async fn maybe_report(base_dir: &PathBuf) -> Result<(), String> {
    let version = env!("CARGO_PKG_VERSION");

    if !stats_enabled(base_dir) {
        return Err("统计已关闭(设置-隐私)".into());
    }
    let last = last_report_ms(base_dir);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if last != 0 && now.saturating_sub(last) < MIN_INTERVAL_MS {
        return Err("今日已上报".into());
    }

    let payload = json!({
        "kind": "stats",
        "anonId": anon_id(),
        "fields": Value::Object(collect_fields(version)),
    });
    let envelope = crate::feedback::build_envelope(&payload)?;
    crate::feedback::post_envelope(API_URL.to_string(), envelope).await?;
    write_last_report(base_dir);
    Ok(())
}
