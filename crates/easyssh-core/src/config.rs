//! config.json / config.key 读写（与 3.x 目录布局兼容）。
//! 3.x 布局：exe 同目录下 config.json / config.key(.enc/.bak) / keys/ / logs/。
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::crypto::{decrypt_secret, encrypt_secret};

pub const ENC_PREFIX: &str = "enc:v1:";
const SECRET_FIELDS: [&str; 2] = ["password", "passphrase"];
// R-20261122-001 端口敲门 SPA 密钥:与会话密码同级加密落盘,位于会话记录
// knock.{key,hmacKey}(平铺布局),绝不以明文写入 config.json。
const KNOCK_SPA_SECRET_FIELDS: [&str; 2] = ["key", "hmacKey"];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub sessions: Value,
    #[serde(default)]
    pub settings: Value,
}

pub struct ConfigStore {
    pub dir: PathBuf, // exe 同目录（3.x 兼容布局根）
}

impl ConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config.json")
    }
    pub fn key_file(&self) -> PathBuf {
        self.dir.join("config.key")
    }
    pub fn key_enc_file(&self) -> PathBuf {
        self.dir.join("config.key.enc")
    }
    pub fn key_bak_file(&self) -> PathBuf {
        self.dir.join("config.key.bak")
    }
    pub fn keys_dir(&self) -> PathBuf {
        self.dir.join("keys")
    }

    /// 主密钥 DEK：不存在 config.key.enc 时从 config.key 读取(缺失则生成)
    /// v4.3.1 安全加固：config.key 落盘用 Windows DPAPI 绑定当前用户——拷贝
    /// config.json+config.key 到其他用户/机器无法再解密保存的密码。
    /// 兼容读旧版明文 32B 格式（读到后透明升级为 DPAPI 保护）；
    /// DPAPI 文件在本机解不开（换用户/换机器）时明确报错，绝不静默重新生成。
    pub fn secret_key(&self) -> Result<Vec<u8>, String> {
        let f = self.key_file();
        if let Ok(raw) = fs::read(&f) {
            if raw.starts_with(dpapi::MAGIC) {
                return dpapi::unwrap_file_bytes(&raw).ok_or_else(|| {
                    "config.key 已绑定原来的电脑用户(数据保护)，无法在本机解密已保存的密码。\
如需在本机继续使用：删除 config.key(旧密码不可恢复，需重新录入)，或先在原电脑上启用主密码后再迁移。"
                        .to_string()
                });
            }
            if raw.len() == 32 {
                // 旧版明文密钥：透明升级为 DPAPI 保护（升级失败不影响本次使用）
                if let Ok(protected) = dpapi::protect_file_bytes(&raw) {
                    let _ = write_file_atomic(&f, &protected);
                }
                return Ok(raw);
            }
        }
        let mut k = vec![0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut k);
        // v4.3.0：密钥文件写入原子化（tmp→rename），避免断电/崩溃留下残缺密钥
        let data = dpapi::protect_file_bytes(&k).unwrap_or_else(|_| k.clone());
        write_file_atomic(&f, &data).map_err(|e| format!("写入 config.key 失败: {e}"))?;
        Ok(k)
    }

    /// v4.3.1：主密码关闭还原 config.key 时同样走 DPAPI 保护（与 secret_key 格式一致）
    pub fn write_key_file(&self, key: &[u8]) -> Result<(), String> {
        let data = dpapi::protect_file_bytes(key).unwrap_or_else(|_| key.to_vec());
        write_file_atomic(&self.key_file(), &data)
    }

    /// 统一「取 DEK」入口（与 3.x getDek 一致）：
    /// 启用主密码且未解包 → Err(需解锁)；否则读/生成 config.key。
    pub fn get_dek(&self, unwrapped: Option<&[u8]>) -> Result<Vec<u8>, String> {
        if let Some(k) = unwrapped {
            return Ok(k.to_vec());
        }
        if self.key_enc_file().exists() {
            return Err("主密码未解锁".into());
        }
        self.secret_key()
    }

    pub fn masterpw_enabled(&self) -> bool {
        self.key_enc_file().exists()
    }

    pub fn load(&self) -> Result<Config, String> {
        self.load_with_dek(None)
    }

    /// dek: 已解包的 DEK（主密码解锁后由调用方传入）。None 时：
    ///   - 未启用主密码 → 用 config.key 解密；
    ///   - 已启用主密码但未解锁 → 字段保持密文原样返回（绝不破坏数据，渲染端解锁后重新拉取）。
    pub fn load_with_dek(&self, dek: Option<&[u8]>) -> Result<Config, String> {
        // v4.3.1 健壮性：主文件缺失/为空/损坏时依次回退 bak.1~bak.5，全部无效则按空配置启动
        let mut raw = None;
        let candidates = std::iter::once(self.config_file())
            .chain((1..=5).map(|i| self.dir.join(format!("config.json.{}.{}", "bak", i))));
        for path in candidates {
            if let Ok(s) = fs::read_to_string(&path) {
                let trimmed = s.trim();
                if !trimmed.is_empty() && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
                    raw = Some(s);
                    break;
                }
            }
        }
        let raw = raw.unwrap_or_else(|| "{}".into());
        let mut cfg: Config = serde_json::from_str(&raw).map_err(|e| format!("config.json 解析失败: {e}"))?;
        let effective = match dek {
            Some(k) => Some(k.to_vec()),
            None => match self.get_dek(None) {
                Ok(k) => Some(k),
                Err(_) => None,
            },
        };
        if let Some(dek) = effective {
            decrypt_sessions_secrets(&mut cfg.sessions, |v| decrypt_secret(v, &dek));
        }
        Ok(cfg)
    }

    pub fn save(&self, cfg: &mut Config, unwrapped: Option<&[u8]>) -> Result<(), String> {
        let dek = match self.get_dek(unwrapped) {
            Ok(k) => k,
            Err(_) => return Err("主密码未解锁".into()),
        };
        encrypt_sessions_secrets(&mut cfg.sessions, |v| encrypt_secret(v, &dek));
        let data = serde_json::to_string_pretty(&json!({
            "sessions": cfg.sessions,
            "settings": cfg.settings,
        })).map_err(|e| e.to_string())?;
        // v4.3.0 健壮性强化：原子写（tmp→rename，杜绝半写损坏）+ 滚动备份（保留最近 5 份）
        write_file_atomic(&self.config_file(), data.as_bytes())
            .map_err(|e| format!("写入 config.json 失败: {e}"))?;
        rotate_backups(&self.config_file(), "bak", 5);
        Ok(())
    }
}

fn decrypt_sessions_secrets(sessions: &mut Value, f: impl Fn(&str) -> Result<String, String>) {
    if let Some(arr) = sessions.as_array_mut() {
        for s in arr {
            for field in SECRET_FIELDS {
                crypt_secret_field(s, field, true, &f);
            }
            // R-20261122-001:knock.{key,hmacKey} 嵌套路径同样加解密
            if let Some(knock) = s.get_mut("knock") {
                for field in KNOCK_SPA_SECRET_FIELDS {
                    crypt_secret_field(knock, field, true, &f);
                }
            }
        }
    }
}

fn encrypt_sessions_secrets(sessions: &mut Value, f: impl Fn(&str) -> Result<String, String>) {
    if let Some(arr) = sessions.as_array_mut() {
        for s in arr {
            for field in SECRET_FIELDS {
                crypt_secret_field(s, field, false, &f);
            }
            if let Some(knock) = s.get_mut("knock") {
                for field in KNOCK_SPA_SECRET_FIELDS {
                    crypt_secret_field(knock, field, false, &f);
                }
            }
        }
    }
}

/// 单字段加/解密(带 ENC_PREFIX 幂等守卫;decrypt=true 解密,false 加密)。
fn crypt_secret_field(
    obj: &mut Value,
    field: &str,
    decrypt: bool,
    f: &impl Fn(&str) -> Result<String, String>,
) {
    if let Some(v) = obj.get(field).and_then(|x| x.as_str()) {
        let starts_enc = v.starts_with(ENC_PREFIX);
        let hit = if decrypt { starts_enc } else { !v.is_empty() && !starts_enc };
        if hit {
            if let Ok(plain) = f(v) {
                obj[field] = json!(plain);
            }
        }
    }
}

/// 生成 3.x 风格 config.key（32 随机字节）
pub fn generate_dek() -> Vec<u8> {
    use rand::RngCore;
    let mut k = vec![0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    k
}

pub fn write_file_atomic(p: &Path, data: &[u8]) -> Result<(), String> {
    let mut tmp = p.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(data).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, p).map_err(|e| e.to_string())
}

/// 滚动备份：写入成功后调用，`config.json.bak.5←4←3←2←1←当前`，超出 keep 份的删除。
/// ext 为 "bak"，产物为 config.json.bak.1 ~ .N。
fn rotate_backups(p: &Path, ext: &str, keep: u32) {
    let bak = |n: u32| -> PathBuf {
        let mut s = p.as_os_str().to_os_string();
        s.push(format!(".{}.{}", ext, n));
        PathBuf::from(s)
    };
    let old = bak(keep);
    if old.exists() {
        let _ = fs::remove_file(&old);
    }
    for n in (1..keep).rev() {
        let _ = fs::rename(bak(n), bak(n + 1));
    }
    let _ = fs::copy(p, bak(1));
}

/* ---------- v4.3.1 安全加固：config.key DPAPI 绑定当前用户 ---------- */
/// 文件格式：前缀 "ESSHDPAPI1\n" + CryptProtectData 密文（可选熵绑定应用标识）。
pub mod dpapi {
    pub const MAGIC: &[u8] = b"ESSHDPAPI1\n";
    #[cfg(windows)]
    const ENTROPY: &[u8] = b"EasySSH v4 config.key binding";

    /// 识别并解包 DPAPI 保护格式；非该格式返回 None（调用方按旧格式处理）
    pub fn unwrap_file_bytes(raw: &[u8]) -> Option<Vec<u8>> {
        if !raw.starts_with(MAGIC) {
            return None;
        }
        #[cfg(windows)]
        {
            unprotect(&raw[MAGIC.len()..]).ok()
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn protect_file_bytes(key: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = MAGIC.to_vec();
        #[cfg(windows)]
        out.extend_from_slice(&protect(key)?);
        #[cfg(not(windows))]
        out.extend_from_slice(key);
        Ok(out)
    }

    #[cfg(windows)]
    fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{LocalFree, HLOCAL};
        use windows::Win32::Security::Cryptography::{
            CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        };
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let entropy = CRYPT_INTEGER_BLOB {
                cbData: ENTROPY.len() as u32,
                pbData: ENTROPY.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptProtectData(
                &input,
                PCWSTR::null(),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .map_err(|e| format!("DPAPI 加密失败: {e}"))?;
            let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(output.pbData as *mut _));
            Ok(out)
        }
    }

    #[cfg(windows)]
    fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
        use windows::Win32::Foundation::{LocalFree, HLOCAL};
        use windows::Win32::Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        };
        unsafe {
            let input = CRYPT_INTEGER_BLOB {
                cbData: data.len() as u32,
                pbData: data.as_ptr() as *mut u8,
            };
            let entropy = CRYPT_INTEGER_BLOB {
                cbData: ENTROPY.len() as u32,
                pbData: ENTROPY.as_ptr() as *mut u8,
            };
            let mut output = CRYPT_INTEGER_BLOB::default();
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
            .map_err(|e| format!("DPAPI 解密失败(可能已换用户/机器): {e}"))?;
            let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            let _ = LocalFree(HLOCAL(output.pbData as *mut _));
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_and_rotate() {
        let dir = std::env::temp_dir().join(format!("easyssh-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.json");
        for i in 0..8u32 {
            write_file_atomic(&p, format!("v{i}").as_bytes()).unwrap();
            assert_eq!(fs::read_to_string(&p).unwrap(), format!("v{i}"));
            rotate_backups(&p, "bak", 5);
        }
        assert_eq!(fs::read_to_string(&p).unwrap(), "v7");
        assert_eq!(fs::read_to_string(dir.join("config.json.bak.1")).unwrap(), "v7");
        // 只保留 5 份，bak.6+ 不存在
        assert!(!dir.join("config.json.bak.6").exists());
        assert!(!PathBuf::from(format!("{}.tmp", p.display())).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn knock_spa_secret_fields_roundtrip() {
        // R-20261122-001:knock.spa.key/hmacKey 加密落盘 → 解密还原(幂等)
        let dek = [7u8; 32];
        let mut sessions = serde_json::json!([
            {
                "host": "h", "port": 22, "username": "u",
                "knock": { "enabled": true, "type": "spa", "spaPort": 62201,
                    "sequence": [], "key": "raw-spa-key", "hmacKey": "raw-hmac-key",
                    "openPorts": "tcp/22", "fwTimeout": 30 }
            }
        ]);
        encrypt_sessions_secrets(&mut sessions, |v| encrypt_secret(v, &dek));
        let enc_key = sessions[0]["knock"]["key"].as_str().unwrap();
        let enc_hmac = sessions[0]["knock"]["hmacKey"].as_str().unwrap();
        assert!(enc_key.starts_with(ENC_PREFIX), "knock.key 未加密: {enc_key}");
        assert!(enc_hmac.starts_with(ENC_PREFIX));
        assert!(!enc_key.contains("raw-spa-key"));
        // 幂等:再加密一次不变
        let enc_key_snapshot = enc_key.to_string();
        encrypt_sessions_secrets(&mut sessions, |v| encrypt_secret(v, &dek));
        assert_eq!(sessions[0]["knock"]["key"].as_str().unwrap(), enc_key_snapshot);
        decrypt_sessions_secrets(&mut sessions, |v| decrypt_secret(v, &dek));
        assert_eq!(sessions[0]["knock"]["key"], "raw-spa-key");
        assert_eq!(sessions[0]["knock"]["hmacKey"], "raw-hmac-key");
    }
}
