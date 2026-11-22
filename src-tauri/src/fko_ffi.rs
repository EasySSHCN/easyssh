// fko_ffi.rs — R-20261122-002: libfko (fwknop 2.6.10) 最小 FFI 绑定。
// 仅暴露构造 SPA 包所需函数；库以 cc crate 静态编入本 crate（见 build.rs）。
// 常量值与 vendor-libfko/fko.h 中枚举定义逐一对应（改库必须同步改这里）。
#![allow(non_snake_case, dead_code)]

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_short, c_void};

pub type fko_ctx_t = *mut c_void;

/// fko.h fko_digest_type_t：FKO_DIGEST_SHA256 = 3
pub const FKO_DIGEST_SHA256: c_short = 3;
/// fko.h fko_hmac_type_t：FKO_HMAC_SHA256 = 3
pub const FKO_HMAC_SHA256: c_short = 3;
/// fko.h：FKO_SUCCESS = 0
pub const FKO_SUCCESS: c_int = 0;

extern "C" {
    pub fn fko_new(ctx: *mut fko_ctx_t) -> c_int;
    pub fn fko_destroy(ctx: fko_ctx_t) -> c_int;
    pub fn fko_set_rand_value(ctx: fko_ctx_t, val: *const c_char) -> c_int;
    pub fn fko_set_spa_message(ctx: fko_ctx_t, msg_string: *const c_char) -> c_int;
    pub fn fko_set_spa_client_timeout(ctx: fko_ctx_t, timeout: c_int) -> c_int;
    pub fn fko_set_spa_digest_type(ctx: fko_ctx_t, digest_type: c_short) -> c_int;
    pub fn fko_set_spa_hmac_type(ctx: fko_ctx_t, hmac_type: c_short) -> c_int;
    pub fn fko_spa_data_final(
        ctx: fko_ctx_t,
        enc_key: *const c_char,
        enc_key_len: c_int,
        hmac_key: *const c_char,
        hmac_key_len: c_int,
    ) -> c_int;
    pub fn fko_get_spa_data(ctx: fko_ctx_t, spa_data: *mut *mut c_char) -> c_int;
    pub fn fko_errstr(err_code: c_int) -> *const c_char;
}

/// 取 libfko 错误码对应描述（fallback 到数字）。
pub fn fko_err(code: c_int) -> String {
    unsafe {
        let p = fko_errstr(code);
        if p.is_null() {
            return format!("fko error {code}");
        }
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
