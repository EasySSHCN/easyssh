//! v5(M1)/F-M1-01：OTA 升级清单 Ed25519 非对称签名与验签。
//!
//! 设计要点（见 docs/v5/产品开发手册.md §3.1、S-OTA-01）：
//! - 签名对象是升级清单中 exe 条目的关键字段（规范化字符串），即间接覆盖安装包
//!   SHA-256 摘要；私钥仅发布机持有，公钥编译期嵌入客户端（见 src-tauri/src/updater.rs）。
//! - 清单新增 `ed25519` 字段（64 字节签名的 base64）；无该字段时进入 v4 老通道
//!   兼容窗口（警告但放行），5.0 正式版起强制（调用方按 `strict` 控制）。
//! - 规范化消息版本 V1，字段缺失/类型不符直接报错，绝不静默放行。
//!
//! 本模块不执行网络操作，纯数据签名/验签，客户端与离线签名工具共用同一实现。
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// 规范化消息协议版本（如需变更字段集，递增此编号并同步签名工具与客户端）
pub const OTA_SIG_PROTOCOL: &str = "ESSH-UPDATE-MANIFEST-SIG-V1";
/// 清单中的签名字段名
pub const OTA_SIG_FIELD: &str = "ed25519";

/// 从升级清单提取的、纳入签名的关键字段
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaManifestFields {
    pub latest_version: String,
    pub released_at: String,
    pub file: String,
    pub sha256: String,
    pub size: u64,
    pub enc_url: String,
}

impl OtaManifestFields {
    /// 规范化待签名字符串：`协议行 + 字段行`，字段顺序固定；
    /// 签名工具与客户端必须逐字节一致。
    pub fn canonical_message(&self) -> Vec<u8> {
        format!(
            "{}\nlatestVersion={}\nreleasedAt={}\nfile={}\nsha256={}\nsize={}\nencUrl={}\n",
            OTA_SIG_PROTOCOL,
            self.latest_version,
            self.released_at,
            self.file,
            self.sha256.to_ascii_lowercase(),
            self.size,
            self.enc_url,
        )
        .into_bytes()
    }
}

/// 从清单 JSON（update manifest）解析 exe 条目关键字段。
/// exe 条目选取规则与 update_flow::do_check 一致：kind=exe 或文件名 .exe 且有 encUrl。
pub fn parse_manifest_fields(manifest: &serde_json::Value) -> Result<OtaManifestFields, String> {
    let latest_version = manifest
        .get("latestVersion")
        .and_then(|x| x.as_str())
        .ok_or("清单缺少 latestVersion")?
        .to_string();
    let released_at = manifest
        .get("releasedAt")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let entry = manifest
        .get("files")
        .and_then(|f| f.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|f| {
                    (f.get("kind").and_then(|k| k.as_str()) == Some("exe")
                        || f.get("file")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_lowercase().ends_with(".exe"))
                            .unwrap_or(false))
                        && f.get("encUrl").and_then(|x| x.as_str()).is_some()
                })
                .cloned()
        })
        .ok_or("清单缺少 exe 安装包条目")?;
    let g = |k: &str| -> Result<String, String> {
        entry
            .get(k)
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or(format!("清单 exe 条目缺少 {k}"))
    };
    Ok(OtaManifestFields {
        latest_version,
        released_at,
        file: g("file")?,
        sha256: g("sha256")?,
        size: entry.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        enc_url: g("encUrl")?,
    })
}

/// 读取清单中的签名值（64 字节签名的 base64）；清单无签名字段返回 Ok(None)
/// （调用方据此进入 v4 兼容窗口或按 strict 拒绝）。
pub fn manifest_signature(manifest: &serde_json::Value) -> Result<Option<Vec<u8>>, String> {
    match manifest.get(OTA_SIG_FIELD) {
        None => Ok(None),
        Some(v) => {
            let s = v
                .as_str()
                .ok_or(format!("{OTA_SIG_FIELD} 字段必须是 base64 字符串"))?;
            let sig = b64d(s).map_err(|e| format!("{OTA_SIG_FIELD} 字段 base64 解码失败: {e}"))?;
            if sig.len() != 64 {
                return Err(format!("{OTA_SIG_FIELD} 签名长度非法({} != 64)", sig.len()));
            }
            Ok(Some(sig))
        }
    }
}

/// 验签：规范化消息 + 清单签名 + 内嵌公钥。失败返回错误（客户端据此拒绝安装）。
pub fn verify_manifest(fields: &OtaManifestFields, sig64: &[u8], pubkey32: &[u8]) -> Result<(), String> {
    let pk: [u8; 32] = pubkey32
        .try_into()
        .map_err(|_| "公钥长度非法(必须 32 字节)".to_string())?;
    let vk = VerifyingKey::from_bytes(&pk).map_err(|e| format!("公钥解析失败: {e}"))?;
    let sig: [u8; 64] = sig64
        .try_into()
        .map_err(|_| "签名长度非法(必须 64 字节)".to_string())?;
    let sig = Signature::from_bytes(&sig);
    vk.verify(&fields.canonical_message(), &sig)
        .map_err(|_| "升级包签名校验失败(清单被篡改或签名无效)".to_string())
}

/// 签名（发布侧 essh-sign 工具使用）：输入 32 字节私钥种子，输出 64 字节签名的 base64。
pub fn sign_manifest(fields: &OtaManifestFields, seed32: &[u8]) -> Result<String, String> {
    let seed: [u8; 32] = seed32
        .try_into()
        .map_err(|_| "私钥种子长度非法(必须 32 字节)".to_string())?;
    let sk = SigningKey::from_bytes(&seed);
    let sig = sk.sign(&fields.canonical_message());
    Ok(b64e(&sig.to_bytes()))
}

/// 由私钥种子推导公钥（工具 --keygen 与指纹展示共用）
pub fn pubkey_from_seed(seed32: &[u8]) -> Result<[u8; 32], String> {
    let seed: [u8; 32] = seed32
        .try_into()
        .map_err(|_| "私钥种子长度非法(必须 32 字节)".to_string())?;
    Ok(SigningKey::from_bytes(&seed).verifying_key().to_bytes())
}

/// 公钥指纹：SHA-256(公钥) 前 16 hex 字符（设置页展示用）
pub fn pubkey_fingerprint(pubkey32: &[u8]) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(pubkey32);
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// 随机生成 Ed25519 密钥对（工具 --keygen 使用）
pub fn generate_keypair() -> Result<(Vec<u8> /*seed*/, [u8; 32] /*pubkey*/), String> {
    use rand::RngCore;
    let mut seed = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let pk = pubkey_from_seed(&seed)?;
    Ok((seed, pk))
}

fn b64d(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(s)
}
fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields() -> OtaManifestFields {
        OtaManifestFields {
            latest_version: "5.0.0".into(),
            released_at: "2026-11-03".into(),
            file: "EasySSH-5.0.0-portable.exe".into(),
            sha256: "abcdef0123456789".into(),
            size: 19_876_543,
            enc_url: "dl/v5/EasySSH-5.0.0-portable.exe.enc".into(),
        }
    }

    #[test]
    fn sign_verify_roundtrip_and_tamper() {
        let (seed, pk) = generate_keypair().unwrap();
        let f = fields();
        let sig = sign_manifest(&f, &seed).unwrap();
        assert!(verify_manifest(&f, &b64d(&sig).unwrap(), &pk).is_ok());
        // 篡改任一字段 → 验签失败
        let mut tampered = f.clone();
        tampered.sha256 = "0000".into();
        assert!(verify_manifest(&tampered, &b64d(&sig).unwrap(), &pk).is_err());
        // sha256 大小写归一化：大写摘要签名可被小写清单验证（服务端大小写漂移容忍）
        let mut lower = f.clone();
        lower.sha256 = f.sha256.to_uppercase();
        assert!(verify_manifest(&lower, &b64d(&sig).unwrap(), &pk).is_ok());
    }

    #[test]
    fn parse_manifest_and_signature_field() {
        let (seed, pk) = generate_keypair().unwrap();
        let f = fields();
        let sig = sign_manifest(&f, &seed).unwrap();
        let manifest = json!({
            "latestVersion": "5.0.0",
            "releasedAt": "2026-11-03",
            "files": [{
                "kind": "exe",
                "file": "EasySSH-5.0.0-portable.exe",
                "sha256": "abcdef0123456789",
                "size": 19876543,
                "encUrl": "dl/v5/EasySSH-5.0.0-portable.exe.enc",
            }],
            "ed25519": sig,
        });
        let parsed = parse_manifest_fields(&manifest).unwrap();
        assert_eq!(parsed, f);
        let s = manifest_signature(&manifest).unwrap().unwrap();
        assert!(verify_manifest(&parsed, &s, &pk).is_ok());
        // 无签名字段 → None（兼容窗口判定依据）
        let mut legacy = manifest.clone();
        legacy.as_object_mut().unwrap().remove(OTA_SIG_FIELD);
        assert!(manifest_signature(&legacy).unwrap().is_none());
        // 缺字段清单 → 解析失败
        assert!(parse_manifest_fields(&json!({})).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        let (seed, _) = generate_keypair().unwrap();
        let (_, other_pk) = generate_keypair().unwrap();
        let f = fields();
        let sig = sign_manifest(&f, &seed).unwrap();
        // 攻击者自签（公钥不同）→ 拒绝
        assert!(verify_manifest(&f, &b64d(&sig).unwrap(), &other_pk).is_err());
    }
}
