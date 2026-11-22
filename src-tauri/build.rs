/* 构建脚本：除 tauri_build 外，把升级主密钥文件在构建期嵌入二进制。
 * 密钥文件不入源码仓库：按顺序探测 EASYSSH_UPDATE_KEYS 环境变量与若干相对路径，
 * 都不存在时嵌入空串（公开构建），updater/feedback 在运行时报“密钥未配置”。 */
fn main() {
    tauri_build::build();

    // v4.2.4:构建时间戳(用于 UI 缓存标记——每次重新构建必然触发一次 WebView2 缓存清理)
    let build_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=EASYSSH_BUILD_ID={build_id}");

    // 嵌入升级主密钥（官方构建携带；公开构建无密钥文件时嵌入空串）
    let mut content = String::new();
    if let Ok(p) = std::env::var("EASYSSH_UPDATE_KEYS") {
        if let Ok(s) = std::fs::read_to_string(&p) {
            content = s;
        }
    }
    if content.trim().is_empty() {
        for p in ["../../update-keys.json", "../update-keys.json", "update-keys.json"] {
            if let Ok(s) = std::fs::read_to_string(p) {
                if !s.trim().is_empty() {
                    content = s;
                    break;
                }
            }
        }
    }
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    std::fs::write(
        format!("{out}/embedded_update_keys.rs"),
        format!("{:?}", content),
    )
    .expect("write embedded keys");

    // R-20261122-002: 把 vendored libfko（fwknop 2.6.10，来源 /tmp/fwknop-2.6.10，
    // 上游 github.com/mrsteyn/libfwknop / Debian fwknop_2.6.10）静态编入二进制。
    // 排除 server/pcap 与 gpgme/fko_utests；WIN32 由 fwknop 源内条件分支使用。
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let fko = std::path::Path::new(&manifest).join("vendor-libfko");
    let mut build = cc::Build::new();
    build
        .include(&fko)
        .define("HAVE_CONFIG_H", None)
        .define("SHA2_USE_MEMSET_MEMCPY", None) // sha2.c 默认走 bzero/bcopy，MSVC 需 memset 路径
        .define("HAVE_MEMSET", None) // fko_common.h bzero 兜底宏开关
        .warnings(false)
        .opt_level(2);
    if cfg!(windows) {
        build.define("WIN32", None); // fwknop 源内 Win32 随机源/timeb 分支开关
        // MSVC 下 getlogin.c 使用 advapi32!GetUserNameA（fwknop win32/ 移植 shim）
        println!("cargo:rustc-link-lib=advapi32");
    }
    for f in [
        "base64.c", "cipher_funcs.c", "digest.c",
        "fko_client_timeout.c", "fko_decode.c", "fko_digest.c", "fko_encode.c",
        "fko_encryption.c", "fko_error.c", "fko_funcs.c", "fko_hmac.c",
        "fko_message.c", "fko_nat_access.c", "fko_rand_value.c",
        "fko_server_auth.c", "fko_timestamp.c", "fko_user.c", "getlogin.c", "fko_util.c",
        "hmac.c", "md5.c", "rijndael.c", "sha1.c", "sha2.c", "sha3.c",
        "strlcat.c", "strlcpy.c",
    ] {
        build.file(fko.join(f));
    }
    build.compile("fko");
}
