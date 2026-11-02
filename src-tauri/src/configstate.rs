//! 配置状态：3.x 兼容的 config.json / config.key / knownHosts / 主密码生命周期管理。
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

use easyssh_core::config::{Config, ConfigStore};
use easyssh_core::crypto::{unwrap_dek, wrap_dek};

/// 主密码状态（与 3.x 一致）：已解包 DEK（内存）/ 连续失败计数
pub struct ConfigState {
    pub store: ConfigStore,
    unwrapped: Mutex<Option<Vec<u8>>>,
    fail_count: Mutex<u32>,
}

pub fn default_base_dir() -> PathBuf {
    // 3.x 便携语义：exe 同目录即配置根目录
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

impl ConfigState {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            store: ConfigStore::new(dir),
            unwrapped: Mutex::new(None),
            fail_count: Mutex::new(0),
        }
    }

    fn dek(&self) -> Result<Vec<u8>, String> {
        let g = self.unwrapped.lock().unwrap_or_else(|e| e.into_inner());
        self.store.get_dek(g.as_deref())
    }

    fn set_dek(&self, v: Option<Vec<u8>>) {
        *self.unwrapped.lock().unwrap_or_else(|e| e.into_inner()) = v;
    }

    /// 读取 config.json（解密密码字段后返回给渲染端）。
    /// v4 修复：使用进程内已解包 DEK——启用主密码后 config.key 已被 .bak 包裹，
    /// 必须用内存中的 DEK 解密（与 3.x getDek 行为一致），否则渲染端拿到密文。
    pub fn load_value(&self) -> Result<Value, String> {
        let dek = self.unwrapped.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let cfg = self.store.load_with_dek(dek.as_deref())?;
        Ok(json!({ "sessions": cfg.sessions, "settings": cfg.settings }))
    }

    /// 保存 config.json（密码字段自动加密落盘）
    pub fn save_value(&self, sessions: Value, settings: Value) -> Result<(), String> {
        let dek = self.dek()?;
        let mut cfg = Config { sessions, settings };
        self.store.save(&mut cfg, Some(&dek))
    }

    /* ---------- 已知主机指纹 ---------- */
    pub fn knownhosts_get(&self, host: String, port: u16) -> Option<String> {
        let cfg = self.store.load().ok()?;
        cfg.settings
            .get("knownHosts")?
            .get(format!("{}:{}", host, port))?
            .as_str()
            .map(|s| s.to_string())
    }

    pub fn knownhosts_record(&self, host: String, port: u16, fp: String) -> Result<(), String> {
        let mut cfg = self.store.load()?;
        let settings = cfg.settings.as_object_mut().ok_or("settings 非对象")?;
        let kh = settings.entry("knownHosts").or_insert_with(|| json!({}));
        kh.as_object_mut()
            .ok_or("knownHosts 非对象")?
            .insert(format!("{}:{}", host, port), json!(fp));
        let dek = self.dek()?;
        self.store.save(&mut cfg, Some(&dek))
    }

    /* ---------- 主密码 ---------- */
    pub fn masterpw_status(&self) -> Value {
        json!({ "enabled": self.store.masterpw_enabled() })
    }

    pub fn masterpw_unlock(&self, pw: &str) -> Result<(), String> {
        if *self.fail_count.lock().unwrap_or_else(|e| e.into_inner()) >= 5 {
            return Err("错误次数过多,已锁定".into());
        }
        let raw = std::fs::read_to_string(self.store.key_enc_file())
            .map_err(|_| "主密码文件不存在".to_string())?;
        match unwrap_dek(raw.trim(), pw) {
            Ok(dek) => {
                // v4.3.0 安全强化：旧 wrap:v1(100k 迭代) 解锁成功后透明迁移到 wrap:v2(600k)
                if easyssh_core::crypto::wrap_version(raw.trim()) == Some(1) {
                    if let Ok(wrapped) = easyssh_core::crypto::wrap_dek(&dek, pw) {
                        let _ = easyssh_core::config::write_file_atomic(
                            &self.store.key_enc_file(),
                            wrapped.as_bytes(),
                        );
                    }
                }
                self.set_dek(Some(dek));
                *self.fail_count.lock().unwrap_or_else(|e| e.into_inner()) = 0;
                Ok(())
            }
            Err(e) => {
                let mut fc = self.fail_count.lock().unwrap_or_else(|e| e.into_inner());
                *fc += 1;
                if *fc >= 5 {
                    return Err("错误次数过多,已锁定".into());
                }
                Err(e)
            }
        }
    }

    pub fn masterpw_enable(&self, pw: &str) -> Result<(), String> {
        if pw.chars().count() < 6 {
            return Err("主密码至少需要 6 位".into());
        }
        if self.store.masterpw_enabled() {
            return Err("已启用".into());
        }
        let dek = self.store.secret_key()?;
        let wrapped = wrap_dek(&dek, pw)?;
        // v4.3.0：密钥包裹文件写入原子化
        easyssh_core::config::write_file_atomic(&self.store.key_enc_file(), wrapped.as_bytes())
            .map_err(|e| format!("写入 config.key.enc 失败: {e}"))?;
        // 原 config.key 改名 .bak（防手滑保留一份）
        let _ = std::fs::rename(self.store.key_file(), self.store.key_bak_file());
        self.set_dek(Some(dek));
        Ok(())
    }

    pub fn masterpw_change(&self, old_pw: &str, new_pw: &str) -> Result<(), String> {
        if new_pw.chars().count() < 6 {
            return Err("新主密码至少需要 6 位".into());
        }
        let raw = std::fs::read_to_string(self.store.key_enc_file())
            .map_err(|_| "主密码文件不存在".to_string())?;
        let dek = unwrap_dek(raw.trim(), old_pw)?;
        let wrapped = wrap_dek(&dek, new_pw)?;
        easyssh_core::config::write_file_atomic(&self.store.key_enc_file(), wrapped.as_bytes())
            .map_err(|e| format!("写入失败: {e}"))?;
        self.set_dek(Some(dek));
        Ok(())
    }

    pub fn masterpw_disable(&self, pw: &str) -> Result<(), String> {
        let raw = std::fs::read_to_string(self.store.key_enc_file())
            .map_err(|_| "主密码文件不存在".to_string())?;
        let dek = unwrap_dek(raw.trim(), pw)?;
        self.store.write_key_file(&dek)?; // v4.3.1：还原的 config.key 同样走 DPAPI 保护
        std::fs::remove_file(self.store.key_enc_file())
            .map_err(|e| format!("删除 config.key.enc 失败: {e}"))?;
        self.set_dek(Some(dek));
        Ok(())
    }

    pub fn masterpw_reset(&self) -> Result<(), String> {
        let _ = std::fs::remove_file(self.store.key_enc_file());
        let _ = std::fs::remove_file(self.store.key_file());
        // 清空已保存会话密码（会话列表保留）
        let mut cfg = self.store.load().unwrap_or_default();
        if let Some(arr) = cfg.sessions.as_array_mut() {
            for s in arr {
                if let Some(o) = s.as_object_mut() {
                    o.insert("password".into(), json!(""));
                    o.insert("passphrase".into(), json!(""));
                }
            }
        }
        self.store
            .save(&mut cfg, Some(&easyssh_core::config::generate_dek()))?;
        self.set_dek(None);
        Ok(())
    }
}
