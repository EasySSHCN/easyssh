/* ---------- P2：fwknop 风格 SPA 单包授权 ---------- */
/* R-20261122-002：包构造已从自研格式切换为 libfko（fwknop 官方 C 库，2.6.10，
 * vendor 于 src-tauri/vendor-libfko/，经 build.rs 静态编译 + fko_ffi.rs FFI），
 * 输出即 fwknop 原生线格式：Rijndael-256-CBC + OpenSSL Salted__ 盐 + 连续
 * 无填充 base64 + 明文摘要 "rand:user:ts:ver:1:<access b64>:<sha256 b64>" +
 * 尾部 HMAC-SHA256，与 fwknopd 逐字节互通（已真机回归验证）。
 *
 * 历史遗留（R-20261122-001 自研格式，已删除）：明文
 * "{openPorts},{spaPort}:{nonce hex}:{ts}"、SHA256(key) 派生 AES-256-CBC、
 * 外层 SHA256 摘要前置的 base64 包——该格式与原生 fwknopd 不互通，密钥为
 * 自研语义（旧配置的 key/hmacKey 不再有效）。
 *
 * 密钥语义（新）：key/hmacKey 为 fwknop 原生语义——
 *  · 32 字节 base64 字符串（fwknop --key-gen 输出样式）→ 解码后的原始 32B
 *    字节作为密钥（等价 fwknop 客户端 --key-base64-* 与服务器 KEY_BASE64:）；
 *  · 其他字符串 → 原样 ASCII 字节作为口令（等价 --key-rijndael 与 KEY:）。
 * 两种语义分别与服务器 access.conf 的 KEY_BASE64:/KEY: 写法一一对应。 */
use base64::Engine;
use std::ffi::CString;
use std::os::raw::c_int;

use crate::fko_ffi as ffi;

/// fwknop 密钥取值：若字符串恰为 32 字节 base64（--key-gen 输出样式），
/// 解码取原始字节；否则按 ASCII 口令原样使用。
fn key_bytes(s: &str) -> Vec<u8> {
    if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(s.trim()) {
        if raw.len() == 32 {
            return raw;
        }
    }
    s.as_bytes().to_vec()
}

/// 构造 SPA 包（fwknop 原生格式 base64 字符串）。
/// key/hmac_key 为 fwknop 原生语义（见模块头注释）；open_ports 如 "tcp/22"
/// （可逗号分隔多个）；timeout 为 fwknop client_timeout（与服务器开门
/// 保持秒数对齐，同 cfg.fw_timeout）。
pub fn build_packet(
    key: &str,
    hmac_key: &str,
    open_ports: &str,
    timeout: u64,
) -> Result<Vec<u8>, String> {
    if key.is_empty() || hmac_key.is_empty() {
        return Err("SPA 密钥/HMAC 密钥为空".into());
    }
    unsafe {
        let mut ctx: ffi::fko_ctx_t = std::ptr::null_mut();
        let rc = ffi::fko_new(&mut ctx);
        if rc != ffi::FKO_SUCCESS {
            return Err(format!("fko_new 失败: {}", ffi::fko_err(rc)));
        }
        let mut ret = Err(String::new());
        'build: {
            let msg = CString::new(format!("127.0.0.1,{open_ports}"))
                .map_err(|_| "open_ports 含 NUL 字符".to_string());
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    ret = Err(e);
                    break 'build;
                }
            };
            let rc = ffi::fko_set_spa_message(ctx, msg.as_ptr());
            if rc != ffi::FKO_SUCCESS {
                ret = Err(format!("fko_set_spa_message 失败({open_ports}): {}", ffi::fko_err(rc)));
                break 'build;
            }
            let rc = ffi::fko_set_spa_client_timeout(ctx, timeout.clamp(0, i32::MAX as u64) as c_int);
            if rc != ffi::FKO_SUCCESS {
                ret = Err(format!("fko_set_spa_client_timeout 失败: {}", ffi::fko_err(rc)));
                break 'build;
            }
            let rc = ffi::fko_set_spa_digest_type(ctx, ffi::FKO_DIGEST_SHA256);
            if rc != ffi::FKO_SUCCESS {
                ret = Err(format!("fko_set_spa_digest_type 失败: {}", ffi::fko_err(rc)));
                break 'build;
            }
            let rc = ffi::fko_set_spa_hmac_type(ctx, ffi::FKO_HMAC_SHA256);
            if rc != ffi::FKO_SUCCESS {
                ret = Err(format!("fko_set_spa_hmac_type 失败: {}", ffi::fko_err(rc)));
                break 'build;
            }
            let kb = key_bytes(key);
            let hb = key_bytes(hmac_key);
            let rc = ffi::fko_spa_data_final(
                ctx,
                kb.as_ptr() as *const _,
                kb.len() as c_int,
                hb.as_ptr() as *const _,
                hb.len() as c_int,
            );
            if rc != ffi::FKO_SUCCESS {
                ret = Err(format!("fko_spa_data_final 失败: {}", ffi::fko_err(rc)));
                break 'build;
            }
            let mut data: *mut std::os::raw::c_char = std::ptr::null_mut();
            let rc = ffi::fko_get_spa_data(ctx, &mut data);
            if rc != ffi::FKO_SUCCESS || data.is_null() {
                ret = Err(format!("fko_get_spa_data 失败: {}", ffi::fko_err(rc)));
                break 'build;
            }
            let s = std::ffi::CStr::from_ptr(data).to_string_lossy().into_owned();
            ret = Ok(s.into_bytes());
        }
        ffi::fko_destroy(ctx);
        ret
    }
}

/// 解析校验（测试/诊断用：仅验证线格式，不回解密文）。
pub fn packet_format_ok(packet: &[u8]) -> Result<(), String> {
    let s = std::str::from_utf8(packet).map_err(|_| "包非 UTF-8/base64".to_string())?;
    if s.contains('=') {
        return Err("fwknop 线格式 base64 不应带 '=' 填充".into());
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/') {
        return Err("包含 base64 字符集之外的字符".into());
    }
    // 密文段（Salted__ b64 剥前缀后）至少 40 字符 + HMAC-SHA256 b64 约 43 字符
    if s.len() < 80 {
        return Err(format!("包过短({} 字符)", s.len()));
    }
    Ok(())
}

/// 密钥生成（等价 fwknop --key-gen）：32 随机字节 base64
pub fn gen_key() -> String {
    use rand::RngCore;
    let mut k = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut k);
    base64::engine::general_purpose::STANDARD.encode(k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_produces_native_wire_format() {
        let p = build_packet("interop-test-key", "interop-test-hmac", "tcp/22", 30).unwrap();
        packet_format_ok(&p).unwrap();
        let s = String::from_utf8(p.clone()).unwrap();
        // fwknop 线格式：无 '=' 填充；密文段 + HMAC 段两段 b64 拼接，长度可为任意模 4
        // （ Salted__ 前缀已剥除，即不以 fwknop 客户端会剥掉的 'U2FsdGVkX1' 开头整体也不在包内校验）
        // 相同输入两次构造应成功（nonce/盐由 libfko 随机化，内容不必相同）
        let q = build_packet("interop-test-key", "interop-test-hmac", "tcp/22", 30).unwrap();
        assert_ne!(p.clone(), q, "nonce/盐随机化下两次包不应相同");
    }

    #[test]
    fn key_semantics_b64_vs_ascii() {
        // 32B base64 → 解码为原始字节；非 b64-32 → ASCII 原样
        let b64k = gen_key();
        assert_eq!(key_bytes(&b64k).len(), 32);
        assert_eq!(key_bytes("plain-passphrase"), b"plain-passphrase");
        assert_eq!(key_bytes("not-base64!!"), b"not-base64!!");
        // 16B base64 不是 --key-gen 样式，仍按 ASCII
        assert_eq!(key_bytes(&base64::engine::general_purpose::STANDARD.encode([1u8; 16])), base64::engine::general_purpose::STANDARD.encode([1u8; 16]).as_bytes());
    }

    #[test]
    fn invalid_inputs_rejected() {
        assert!(build_packet("", "h", "tcp/22", 30).is_err());
        assert!(build_packet("k", "", "tcp/22", 30).is_err());
        // 非法 access 消息被 libfko 拒绝
        assert!(build_packet("k", "h", "not-a-port-spec!!", 30).is_err());
        assert!(build_packet("k", "h", "tcp/99999", 30).is_err());
    }

    #[test]
    fn gen_key_is_32b_base64() {
        let k = gen_key();
        let raw = base64::engine::general_purpose::STANDARD.decode(&k).unwrap();
        assert_eq!(raw.len(), 32);
    }
}
