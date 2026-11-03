//! AES-256-GCM 秘密加密与主密码 DEK 包裹/解包（与 3.x Electron 版协议完全兼容）。
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use rand::RngCore;

pub const ENC_PREFIX: &str = "enc:v1:";
pub const WRAP_PREFIX: &str = "wrap:v1:";
/// v4.3.0 安全强化：PBKDF2 迭代从 100k 提升至 600k（OWASP 2023 推荐），
/// 新格式 `wrap:v2`；v1 继续可解，解锁成功后自动迁移升级（见 unwrap 返回的版本号）。
pub const WRAP_PREFIX_V2: &str = "wrap:v2:";
/// v4.3.1（本机主线合并）：新写入使用 Argon2id（m=64MiB, t=3, p=1）`wrap:v3`；
/// v1(100k) / v2(600k) 继续可解，实现三格式兼容。
pub const WRAP_PREFIX_V3: &str = "wrap:v3:";
pub const PBKDF2_ITER: u32 = 100_000; // v1 兼容（仅解包旧数据）
pub const PBKDF2_ITER_V2: u32 = 600_000;
const ARGON2_M_KIB: u32 = 64 * 1024;
const ARGON2_T: u32 = 3;
const ARGON2_P: u32 = 1;

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn aes(key: &[u8]) -> Result<Aes256Gcm, String> {
    Aes256Gcm::new_from_slice(key).map_err(|e| format!("密钥长度异常: {e}"))
}

/// 加密秘密字段 → `enc:v1:<iv>:<tag>:<密文>`（空串/已是密文原样返回）
pub fn encrypt_secret(plain: &str, dek: &[u8]) -> Result<String, String> {
    if plain.is_empty() || plain.starts_with(ENC_PREFIX) {
        return Ok(plain.to_string());
    }
    let a = aes(dek)?;
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut iv);
    let mut ct = a
        .encrypt(Nonce::from_slice(&iv), Payload::from(plain.as_bytes()))
        .map_err(|e| e.to_string())?;
    let tag = ct.split_off(ct.len() - 16);
    Ok(format!(
        "{}{}:{}:{}",
        ENC_PREFIX,
        b64().encode(iv),
        b64().encode(&tag),
        b64().encode(&ct)
    ))
}

/// 解密秘密字段；非密文（3.x 旧明文）原样返回
pub fn decrypt_secret(v: &str, dek: &[u8]) -> Result<String, String> {
    if !v.starts_with(ENC_PREFIX) {
        return Ok(v.to_string());
    }
    let body = &v[ENC_PREFIX.len()..];
    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() != 3 {
        return Err("enc:v1 格式无效".into());
    }
    let iv = b64().decode(parts[0]).map_err(|e| e.to_string())?;
    let tag = b64().decode(parts[1]).map_err(|e| e.to_string())?;
    let ct = b64().decode(parts[2]).map_err(|e| e.to_string())?;
    let a = aes(dek)?;
    let mut full = ct;
    full.extend_from_slice(&tag);
    let pt = a
        .decrypt(Nonce::from_slice(&iv), full.as_ref())
        .map_err(|_| "解密失败(密钥不匹配或数据被篡改)".to_string())?;
    Ok(String::from_utf8_lossy(&pt).to_string())
}

/// 用口令派生 KEK 并包裹 DEK → `wrap:v3:<salt>:<iv>:<tag>:<密文>`（Argon2id）
/// v5(M1)/F-M1-03：新加密一律使用本格式（Argon2id, m=64MiB, t=3, p=1，附录 D 编号冻结）；
/// v1(100k)/v2(600k) 仍可解，首次解锁后由调用方透明迁移到 v3（不得降级写回旧格式）。
pub fn wrap_dek(dek: &[u8], pw: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut iv);
    let mut kek = [0u8; 32];
    argon2id_kek(pw.as_bytes(), &salt, &mut kek)?;
    let a = aes(&kek)?;
    let mut ct = a
        .encrypt(Nonce::from_slice(&iv), Payload::from(dek))
        .map_err(|e| e.to_string())?;
    let tag = ct.split_off(ct.len() - 16);
    Ok(format!(
        "{}{}:{}:{}:{}",
        WRAP_PREFIX_V3,
        b64().encode(salt),
        b64().encode(iv),
        b64().encode(&tag),
        b64().encode(&ct)
    ))
}

fn argon2id_kek(pw: &[u8], salt: &[u8], out: &mut [u8; 32]) -> Result<(), String> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(32))
        .map_err(|e| format!("Argon2 参数无效: {e}"))?;
    let a = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    a.hash_password_into(pw, salt, out)
        .map_err(|e| format!("Argon2 派生失败: {e}"))
}

fn wrap_dek_v2(dek: &[u8], pw: &str, iter: u32, prefix: &str) -> Result<String, String> {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut iv);
    let mut kek = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw.as_bytes(), &salt, iter, &mut kek);
    let a = aes(&kek)?;
    let mut ct = a
        .encrypt(Nonce::from_slice(&iv), Payload::from(dek))
        .map_err(|e| e.to_string())?;
    let tag = ct.split_off(ct.len() - 16);
    Ok(format!(
        "{}{}:{}:{}:{}",
        prefix,
        b64().encode(salt),
        b64().encode(iv),
        b64().encode(&tag),
        b64().encode(&ct)
    ))
}

/// 检测已包裹密钥的版本：Some(1)=v1(100k，建议迁移)，Some(2)=v2(600k)，Some(3)=v3(Argon2id)，None=格式无效
pub fn wrap_version(wrapped: &str) -> Option<u8> {
    if wrapped.starts_with(WRAP_PREFIX_V3) {
        Some(3)
    } else if wrapped.starts_with(WRAP_PREFIX_V2) {
        Some(2)
    } else if wrapped.starts_with(WRAP_PREFIX) {
        Some(1)
    } else {
        None
    }
}

/// 用口令从 `config.key.enc` 内容解包出 DEK；口令错误(GCM tag 校验失败)返回错误。
/// 三格式兼容：wrap:v1（100k，3.x/v4 旧数据）/ wrap:v2（600k）/ wrap:v3（Argon2id）。
pub fn unwrap_dek(wrapped: &str, pw: &str) -> Result<Vec<u8>, String> {
    let version = wrap_version(wrapped).ok_or("主密码文件格式无效")?;
    let prefix = match version {
        1 => WRAP_PREFIX,
        2 => WRAP_PREFIX_V2,
        3 => WRAP_PREFIX_V3,
        _ => return Err("主密码文件格式无效".into()),
    };
    let body = &wrapped[prefix.len()..];
    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() != 4 {
        return Err("主密码文件格式无效".into());
    }
    let salt = b64().decode(parts[0]).map_err(|e| e.to_string())?;
    let iv = b64().decode(parts[1]).map_err(|e| e.to_string())?;
    let tag = b64().decode(parts[2]).map_err(|e| e.to_string())?;
    let ct = b64().decode(parts[3]).map_err(|e| e.to_string())?;
    let mut kek = [0u8; 32];
    match version {
        1 => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw.as_bytes(), &salt, PBKDF2_ITER, &mut kek),
        2 => pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pw.as_bytes(), &salt, PBKDF2_ITER_V2, &mut kek),
        _ => argon2id_kek(pw.as_bytes(), &salt, &mut kek)?,
    }
    let a = aes(&kek)?;
    let mut full = ct;
    full.extend_from_slice(&tag);
    let dek = a
        .decrypt(Nonce::from_slice(&iv), full.as_ref())
        .map_err(|_| "主密码错误".to_string())?;
    if dek.len() != 32 {
        return Err("解包出的密钥长度异常".into());
    }
    Ok(dek)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_v3_roundtrip() {
        let dek = {
            use rand::RngCore;
            let mut k = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            k
        };
        let wrapped = wrap_dek(&dek, "correct horse").unwrap();
        assert!(wrapped.starts_with(WRAP_PREFIX_V3));
        assert_eq!(wrap_version(&wrapped), Some(3));
        assert_eq!(unwrap_dek(&wrapped, "correct horse").unwrap(), dek);
        assert!(unwrap_dek(&wrapped, "wrong").is_err());
    }

    #[test]
    fn wrap_v2_compat() {
        // 用 v2 参数手工构造服务器版 4.3.0 格式，验证 v2(600k) 可解 + 版本识别
        let dek = {
            use rand::RngCore;
            let mut k = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            k
        };
        let v2 = wrap_dek_v2(&dek, "v2pw", PBKDF2_ITER_V2, WRAP_PREFIX_V2).unwrap();
        assert_eq!(wrap_version(&v2), Some(2));
        assert_eq!(unwrap_dek(&v2, "v2pw").unwrap(), dek);
        assert!(unwrap_dek(&v2, "wrongpw").is_err());
    }

    #[test]
    fn wrap_v2_to_v3_transparent_migration() {
        // v5(M1)/F-M1-03：v2(PBKDF2-600k) 首次解锁后透明迁移为 v3(Argon2id)，
        // 迁移前后同一 DEK 可解，错误密码在两个格式上均被拒绝
        let dek = {
            use rand::RngCore;
            let mut k = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            k
        };
        let v2 = wrap_dek_v2(&dek, "pw123456", PBKDF2_ITER_V2, WRAP_PREFIX_V2).unwrap();
        assert_eq!(unwrap_dek(&v2, "pw123456").unwrap(), dek);
        // 迁移（调用方语义：解锁成功 → wrap_dek 重包裹）
        let migrated = wrap_dek(&dek, "pw123456").unwrap();
        assert_eq!(wrap_version(&migrated), Some(3));
        assert_eq!(unwrap_dek(&migrated, "pw123456").unwrap(), dek);
        assert!(unwrap_dek(&migrated, "wrongpw").is_err());
        // 旧 v2 在迁移后仍可解（向后兼容读）
        assert_eq!(unwrap_dek(&v2, "pw123456").unwrap(), dek);
    }

    #[test]
    fn wrap_v1_compat_and_migration() {
        // 用 v1 参数手工构造旧格式，验证 v1 可解 + 版本识别
        let dek = {
            use rand::RngCore;
            let mut k = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            k
        };
        let legacy = wrap_dek_v2(&dek, "oldpw", PBKDF2_ITER, WRAP_PREFIX).unwrap();
        assert!(legacy.starts_with(WRAP_PREFIX));
        assert_eq!(wrap_version(&legacy), Some(1));
        assert_eq!(unwrap_dek(&legacy, "oldpw").unwrap(), dek);
        // 迁移：同 DEK 重包裹为 v3 后仍可解
        let migrated = wrap_dek(&dek, "oldpw").unwrap();
        assert_eq!(wrap_version(&migrated), Some(3));
        assert_eq!(unwrap_dek(&migrated, "oldpw").unwrap(), dek);
    }

    #[test]
    fn invalid_format_rejected() {
        assert_eq!(wrap_version("garbage"), None);
        assert!(unwrap_dek("garbage", "x").is_err());
    }

    #[test]
    fn secret_roundtrip() {
        let dek = {
            use rand::RngCore;
            let mut k = vec![0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            k
        };
        let enc = encrypt_secret("s3cret", &dek).unwrap();
        assert!(enc.starts_with(ENC_PREFIX));
        assert_eq!(decrypt_secret(&enc, &dek).unwrap(), "s3cret");
        // 已是密文不二次加密；明文原样返回
        assert_eq!(encrypt_secret(&enc, &dek).unwrap(), enc);
        assert_eq!(decrypt_secret("plain", &dek).unwrap(), "plain");
    }
}
