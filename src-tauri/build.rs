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
}
