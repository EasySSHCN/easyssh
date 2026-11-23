//! AgentToken 落盘存储（方案第 5 节：enroll 换取后 DPAPI / 文件权限 600 落盘）。
//!
//! - Linux/Unix：明文写入文件并强制 0600（`PermissionsExt`），父目录 0700；
//! - Windows：`CryptProtectData`（DPAPI，当前用户作用域）加密后写入
//!   `agent-token.bin`，读取时 `CryptUnprotectData` 解密。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::config::default_token_path;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Crypto(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "token 落盘 IO 失败: {e}"),
            StoreError::Crypto(m) => write!(f, "DPAPI 失败: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// 保存 AgentToken（平台默认路径可被 `path` 覆盖，None 用 [`default_token_path`]）。
pub fn save_token(token: &str, path: Option<&Path>) -> Result<PathBuf, StoreError> {
    let path = path.map(|p| p.to_path_buf()).unwrap_or_else(default_token_path);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
            }
        }
    }
    let blob = protect(token.as_bytes())?;
    write_file_strict(&path, &blob)?;
    Ok(path)
}

/// 读取 AgentToken；不存在返回 None。
pub fn load_token(path: Option<&Path>) -> Result<Option<String>, StoreError> {
    let path = path.map(|p| p.to_path_buf()).unwrap_or_else(default_token_path);
    if !path.is_file() {
        return Ok(None);
    }
    let blob = fs::read(&path)?;
    let plain = unprotect(&blob)?;
    String::from_utf8(plain)
        .map(Some)
        .map_err(|_| StoreError::Crypto("token 解码非 UTF-8".into()))
}

/// 删除落盘 token（logout / 换绑）。
pub fn clear_token(path: Option<&Path>) -> Result<(), StoreError> {
    let path = path.map(|p| p.to_path_buf()).unwrap_or_else(default_token_path);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 平台侧加密：Windows DPAPI / Unix 原样（由文件权限兜底）。
fn protect(plain: &[u8]) -> Result<Vec<u8>, StoreError> {
    #[cfg(windows)]
    {
        dpapi_protect(plain)
    }
    #[cfg(unix)]
    {
        Ok(plain.to_vec())
    }
}

fn unprotect(blob: &[u8]) -> Result<Vec<u8>, StoreError> {
    #[cfg(windows)]
    {
        dpapi_unprotect(blob)
    }
    #[cfg(unix)]
    {
        Ok(blob.to_vec())
    }
}

/// 严格写入：0600（Unix），先写临时文件再 rename（原子替换，服务态不撕裂）。
fn write_file_strict(path: &Path, blob: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension("tmp");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(blob)?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(blob)?;
    }
    fs::rename(&tmp, path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Windows DPAPI
// ---------------------------------------------------------------------

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    fn to_blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let mut out = std::mem::zeroed::<CRYPT_INTEGER_BLOB>();
            let rc = CryptProtectData(
                &to_blob(plain),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut out,
            );
            if rc == 0 {
                return Err("CryptProtectData 失败".into());
            }
            let slice =
                std::slice::from_raw_parts(out.pbData, out.cbData as usize);
            let v = slice.to_vec();
            LocalFree(out.pbData as HLOCAL);
            Ok(v)
        }
    }

    pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>, String> {
        unsafe {
            let mut out = std::mem::zeroed::<CRYPT_INTEGER_BLOB>();
            let mut descr: *mut u16 = std::ptr::null_mut();
            let rc = CryptUnprotectData(
                &to_blob(blob),
                &mut descr,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut out,
            );
            if rc == 0 {
                return Err("CryptUnprotectData 失败（非本用户加密或文件损坏）".into());
            }
            let slice =
                std::slice::from_raw_parts(out.pbData, out.cbData as usize);
            let v = slice.to_vec();
            LocalFree(out.pbData as HLOCAL);
            Ok(v)
        }
    }
}

#[cfg(windows)]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, StoreError> {
    dpapi::protect(plain).map_err(StoreError::Crypto)
}

#[cfg(windows)]
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>, StoreError> {
    dpapi::unprotect(blob).map_err(StoreError::Crypto)
}

// ---------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("easyssh-agent-test-{name}-{}-token", std::process::id()))
    }

    #[test]
    fn token_save_load_roundtrip_with_permissions() {
        let path = tmp_path("rw");
        let _ = clear_token(Some(&path));
        save_token("agent-token-abc", Some(&path)).expect("save");

        // Unix 权限必须是 0600（方案硬性要求）
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "落盘权限必须 0600，实际 {mode:o}");
        }

        assert_eq!(
            load_token(Some(&path)).expect("load"),
            Some("agent-token-abc".to_string())
        );
        clear_token(Some(&path)).expect("clear");
        assert_eq!(load_token(Some(&path)).expect("reload"), None);
    }

    #[test]
    fn overwriting_token_replaces_content() {
        let path = tmp_path("ovr");
        let _ = clear_token(Some(&path));
        save_token("first", Some(&path)).expect("save1");
        save_token("second", Some(&path)).expect("save2");
        assert_eq!(load_token(Some(&path)).expect("load"), Some("second".to_string()));
        clear_token(Some(&path)).expect("clear");
    }

    #[test]
    fn clear_missing_token_is_ok() {
        let path = tmp_path("absent");
        assert!(clear_token(Some(&path)).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_blob_is_opaque_and_user_scoped() {
        let plain = b"secret-token";
        let blob = dpapi::protect(plain).expect("protect");
        assert_ne!(&blob, &plain.to_vec(), "DPAPI 输出不应是明文");
        let back = dpapi::unprotect(&blob).expect("unprotect");
        assert_eq!(back, plain.to_vec());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_file_roundtrip() {
        let path = tmp_path("dpapi");
        let _ = clear_token(Some(&path));
        save_token("dpapi-token", Some(&path)).expect("save");
        let raw = fs::read(&path).expect("raw");
        assert_ne!(&raw, b"dpapi-token", "Windows 落盘必须是 DPAPI 密文");
        assert_eq!(
            load_token(Some(&path)).expect("load"),
            Some("dpapi-token".to_string())
        );
        clear_token(Some(&path)).expect("clear");
    }
}
