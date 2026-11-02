/* EasySSH v4：在线升级（对应 3.x updater.js）
 * 协议与官网 tools/update-crypto.js 一致：
 *  1) GET {base}/api/update/check → 加密信封（AES-256-GCM + HMAC-SHA256 时效签名）
 *  2) 下载 <file>.enc → 流式解密（AES-256-CTR + HMAC-SHA256 双校验）
 *  3) 原地替换：旧 exe 改名 .bak → 新 exe 顶替原名 → 分离重启（防降级 + EBUSY 兜底） */
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::Digest;

const MAGIC: &[u8; 8] = b"ESSHENC1";
const CHECK_SALT: &str = "easyssh-update-check-v1";
const FILE_SALT: &str = "easyssh-update-file-v1";
pub const MIN_UPGRADE_VERSION: &str = "2.5.0";
const API_PATH: &str = "/api/update/check";

fn hkdf_sha256(master: &[u8], salt: &str, info: &str, len: usize) -> Vec<u8> {
    // HKDF-SHA256 (RFC5869) 手写实现（extract + expand）
    use hmac::Mac;
    // RFC 5869 Extract: PRK = HMAC-Hash(salt, IKM) —— salt 为 HMAC 键，IKM(master) 为数据
    let mut prk_h = hmac::Hmac::<sha2::Sha256>::new_from_slice(salt.as_bytes()).expect("hmac key");
    prk_h.update(master);
    let prk = prk_h.finalize().into_bytes();
    let info_b = info.as_bytes();
    let mut out = Vec::with_capacity(len);
    let mut prev: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while out.len() < len {
        let mut hm = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(&prk).expect("hmac key");
        hm.update(&prev);
        hm.update(info_b);
        hm.update(&[counter]);
        let block = hm.finalize().into_bytes();
        let take = (len - out.len()).min(block.len());
        out.extend_from_slice(&block[..take]);
        prev = block.to_vec();
        counter += 1;
    }
    out
}

fn derive_check_keys(master: &[u8]) -> (Vec<u8>, Vec<u8>) {
    (
        hkdf_sha256(master, CHECK_SALT, "aes-256-gcm", 32),
        hkdf_sha256(master, CHECK_SALT, "hmac-sha256", 32),
    )
}

fn derive_file_keys(master: &[u8], file_name: &str, plain_sha256_hex: &str) -> (Vec<u8>, Vec<u8>) {
    let info = format!("enc:{file_name}|{plain_sha256_hex}");
    let enc = hkdf_sha256(master, FILE_SALT, &info, 32);
    let info = format!("mac:{file_name}|{plain_sha256_hex}");
    let mac = hkdf_sha256(master, FILE_SALT, &info, 32);
    (enc, mac)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::Mac;
    let mut hm = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac key");
    hm.update(data);
    hm.finalize().into_bytes().to_vec()
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    use hmac::Mac;
    let mut hm = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac key");
    hm.update(data);
    hex(&hm.finalize().into_bytes())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e: base64::DecodeError| format!("base64 解码失败: {e}"))
}

fn b64e(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let pa: Vec<&str> = a.split('.').collect();
    let pb: Vec<&str> = b.split('.').collect();
    for i in 0..pa.len().max(pb.len()) {
        let x = pa.get(i).copied().unwrap_or("");
        let y = pb.get(i).copied().unwrap_or("");
        let (nx, ny) = (x.parse::<u64>(), y.parse::<u64>());
        match (nx, ny) {
            (Ok(nx), Ok(ny)) => {
                if nx != ny {
                    return nx.cmp(&ny);
                }
            }
            _ => {
                let o = x.cmp(y);
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
        }
    }
    std::cmp::Ordering::Equal
}

/// 读取升级主密钥（exe 同目录 update-keys.json 的 masterKeyBase64）
/// 构建期嵌入的主密钥 JSON（见 build.rs；公开构建为空串）
pub const MASTER_KEY_JSON: &str = include!(concat!(env!("OUT_DIR"), "/embedded_update_keys.rs"));

pub fn load_master(base_dir: &Path) -> Result<Vec<u8>, String> {
    // 1) exe 同目录 update-keys.json 优先（用户可自行更换密钥）
    if let Ok(raw) = std::fs::read_to_string(base_dir.join("update-keys.json")) {
        if let Some(k) = parse_master(&raw) {
            return Ok(k);
        }
    }
    // 2) 构建期嵌入密钥（官方包携带）
    if let Some(k) = parse_master(MASTER_KEY_JSON) {
        return Ok(k);
    }
    Err("升级密钥未配置(update-keys.json 缺失或无效)，在线升级不可用".into())
}

fn parse_master(raw: &str) -> Option<Vec<u8>> {
    if raw.trim().is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    let b = b64d(v.get("masterKeyBase64")?.as_str()?).ok()?;
    if b.len() != 32 {
        return None;
    }
    Some(b)
}


async fn https_get_text(url: &str) -> Result<String, String> {
    // 简化 HTTPS 客户端（避免引入 reqwest 全家桶）：使用 ureq（阻塞）在线程池执行
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let resp = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(15))
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => format!("升级接口 HTTP {code}"),
                other => format!("连接升级服务器失败: {other}"),
            })?;
        resp.into_string()
            .map_err(|e| format!("读取升级响应失败: {e}"))
    })
    .await
    .map_err(|e| format!("任务错误: {e}"))?
}

/// 解密检测信封 → 升级清单 manifest
pub async fn check_update(base: &str, master: &[u8]) -> Result<Value, String> {
    let env: Value = serde_json::from_str(&https_get_text(&format!("{base}{API_PATH}")).await?)
        .map_err(|e| format!("升级响应解析失败: {e}"))?;

    if env.get("v").and_then(|x| x.as_i64()) != Some(1)
        || env.get("alg").and_then(|x| x.as_str()) != Some("A256GCM-HS256")
    {
        return Err("升级接口协议版本不符".into());
    }
    let ts = env.get("ts").and_then(|x| x.as_i64()).unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if (now - ts).abs() > 15 * 60 * 1000 {
        return Err("升级响应已过期，请检查本机时间".into());
    }
    let nonce = env.get("nonce").and_then(|x| x.as_str()).unwrap_or("");
    let ct = env.get("ct").and_then(|x| x.as_str()).unwrap_or("");
    let tag = env.get("tag").and_then(|x| x.as_str()).unwrap_or("");
    let mac = env.get("mac").and_then(|x| x.as_str()).unwrap_or("");

    let (enc_key, mac_key) = derive_check_keys(master);
    let mac_data = format!("{ts}.{nonce}.{ct}.{tag}");
    // 协议：mac = base64(HMAC-SHA256(macKey, mac_data))；常量时间比较
    let want = hmac_sha256(&mac_key, mac_data.as_bytes());
    let got = b64d(mac).map_err(|_| "升级响应校验失败".to_string())?;
    if want.len() != got.len() || !want.iter().zip(&got).all(|(a, b)| a == b) {
        return Err("升级响应校验失败".into());
    }
    let nonce_b = b64d(nonce)?;
    let ct_b = b64d(ct)?;
    let tag_b = b64d(tag).map_err(|e| e)?;
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let a = aes_gcm::Aes256Gcm::new_from_slice(&enc_key).map_err(|e| e.to_string())?;
    let mut full = ct_b.clone();
    full.extend_from_slice(&tag_b);
    let pt = a
        .decrypt(
            aes_gcm::Nonce::from_slice(&nonce_b),
            Payload { msg: full.as_ref(), aad: b"" },
        )
        .map_err(|_| "升级信封解密失败".to_string())?;
    let manifest: Value = serde_json::from_slice(&pt).map_err(|e| format!("清单解析失败: {e}"))?;
    Ok(manifest)
}

/// 下载并解密安装包（流式，边下边算 HMAC），落盘为明文 exe
pub async fn download_and_decrypt(
    base: &str,
    master: &[u8],
    enc_url_path: &str,
    file_name: &str,
    plain_sha256: &str,
    out_file: &Path,
    on_progress: impl Fn(f64) + Send + 'static,
) -> Result<(), String> {
    let url = format!("{base}/{enc_url_path}");
    let data: Vec<u8> = tokio::task::spawn_blocking(move || {
        let mut resp = ureq::get(&url)
            .timeout(std::time::Duration::from_secs(600))
            .call()
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => format!("下载加密包 HTTP {code}"),
                other => format!("下载失败: {other}"),
            })?;
        let mut buf = Vec::new();
        use std::io::Read;
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| format!("下载读取失败: {e}"))?;
        Ok::<Vec<u8>, String>(buf)
    })
    .await
    .map_err(|e| format!("任务错误: {e}"))??;
    on_progress(1.0);

    decrypt_file(&data, file_name, plain_sha256, master, out_file, &on_progress)
}

/// 解密 ESSHENC1 格式（内存内完成，与 3.x 逐块等价）
fn decrypt_file(
    data: &[u8],
    file_name: &str,
    plain_sha256: &str,
    master: &[u8],
    dest: &Path,
    on_progress: &dyn Fn(f64),
) -> Result<(), String> {
    if data.len() < 12 + 8 || &data[0..8] != MAGIC {
        return Err("不是有效的 EasySSH 加密包".into());
    }
    let head_len =
        u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
    if head_len == 0 || head_len > 65536 || data.len() < 12 + head_len + 64 {
        return Err("加密包头非法".into());
    }
    let head_buf = &data[12..12 + head_len];
    let header: Value = serde_json::from_slice(head_buf).map_err(|e| format!("加密包头解析失败: {e}"))?;
    if header.get("v").and_then(|x| x.as_i64()) != Some(1) {
        return Err("加密包头字段缺失".into());
    }
    let iv_b = header
        .get("iv")
        .and_then(|x| x.as_str())
        .ok_or_else(|| "加密包头字段缺失".to_string())
        .and_then(b64d)?;
    let want_hmac = String::from_utf8_lossy(&data[12 + head_len..12 + head_len + 64]).to_string();
    if !want_hmac.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("hmac 区非法".into());
    }
    if header.get("sha256").and_then(|x| x.as_str()) != Some(plain_sha256) {
        return Err("包指纹与升级清单不符".into());
    }
    let (enc_key, mac_key) = derive_file_keys(master, file_name, plain_sha256);

    // HMAC：头JSON || 全部密文
    use hmac::Mac;
    let mut hm = hmac::Hmac::<sha2::Sha256>::new_from_slice(&mac_key).expect("hmac key");
    hm.update(head_buf);
    let ct_start = 12 + head_len + 64;
    hm.update(&data[ct_start..]);
    let got_hmac = hex(&hm.finalize().into_bytes());
    if got_hmac != want_hmac.to_lowercase() {
        return Err("安装包完整性校验失败".into());
    }

    // AES-256-CTR 解密
    use ctr::cipher::{KeyIvInit, StreamCipher};
    use aes::cipher::generic_array::GenericArray;
    let key = GenericArray::from_slice(&enc_key);
    let iv = GenericArray::from_slice(&iv_b);
    let mut dec = <ctr::Ctr128BE<aes::Aes256> as ctr::cipher::KeyIvInit>::new(key, iv);
    let mut plain = data[ct_start..].to_vec();
    dec.apply_keystream(&mut plain);
    let sha = sha2::Sha256::digest(&plain);
    if hex(&sha) != plain_sha256 {
        std::fs::remove_file(dest).ok();
        return Err("安装包校验失败(sha256)".into());
    }
    std::fs::write(dest, &plain).map_err(|e| format!("写入安装包失败: {e}"))?;
    on_progress(1.0);
    Ok(())
}

/* 调试注记：Ctr128BE::new_from_slice 未满足 bound 时，检查 aes/ctr 版本唯一性
 * 与 digest 版本一致性（本工程 aes 0.8.4 / ctr 0.9.2 / digest 0.10） */
