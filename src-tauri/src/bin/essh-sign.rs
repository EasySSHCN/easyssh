/*! v5(M1)/F-M1-01：ESSH 升级清单离线 Ed25519 签名工具（发布机专用）。
 *
 * 用法：
 *   essh-sign --keygen <私钥文件>          生成 Ed25519 密钥对；私钥(base64 种子)写入文件，公钥指纹打到 stdout
 *   essh-sign sign <清单.json> <私钥文件>  读清单 + 私钥 → 清单写入/更新 `ed25519` 字段 → 输出到 stdout（--out 写文件）
 *
 * 私钥文件仅发布机持有（S-OTA-05）：勿入库、勿进源码包/CI。
 * 签名算法与客户端共用 easyssh_core::ota_sign，规范化消息逐字节一致。
 */
use std::io::Write;

fn b64d(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| format!("base64 解码失败: {e}"))
}

fn b64e(b: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(b)
}

/// 私钥文件格式：单行 base64（32 字节种子），允许前置 # 注释行
fn read_seed(path: &str) -> Result<Vec<u8>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("读取私钥文件失败: {e}"))?;
    let line = raw
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or("私钥文件为空")?;
    let seed = b64d(line)?;
    if seed.len() != 32 {
        return Err(format!("私钥种子长度非法({} != 32)", seed.len()));
    }
    Ok(seed)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let err = (|| -> Result<(), String> {
        match args.get(1).map(|s| s.as_str()) {
            Some("--keygen") => {
                let out = args.get(2).ok_or("用法: essh-sign --keygen <私钥文件>")?;
                let (seed, pk) = easyssh_core::ota_sign::generate_keypair()?;
                let fp = easyssh_core::ota_sign::pubkey_fingerprint(&pk);
                let body = format!(
                    "# EasySSH OTA 发布机 Ed25519 私钥（仅发布机持有，勿入库/勿进源码包）\n\
                     # 公钥指纹(前16): {fp}\n\
                     # 客户端内嵌公钥(base64): {}\n",
                    b64e(&pk)
                );
                if std::path::Path::new(out).exists() {
                    return Err(format!(" refusing to overwrite the existing private key file: {out}"));
                }
                std::fs::write(out, format!("{body}{}\n", b64e(&seed)))
                    .map_err(|e| format!("写入私钥文件失败: {e}"))?;
                // 输出客户端需要内嵌的公钥信息
                println!("公钥指纹(前16): {fp}");
                println!("公钥(base64): {}", b64e(&pk));
                println!("公钥(hex): {}", pk.iter().map(|b| format!("{b:02x}")).collect::<String>());
                println!("私钥已写入: {out}");
                Ok(())
            }
            Some("sign") => {
                let manifest_path = args.get(2).ok_or("用法: essh-sign sign <清单.json> <私钥文件> [--out 输出.json]")?;
                let key_path = args.get(3).ok_or("缺少私钥文件参数")?;
                let seed = read_seed(key_path)?;
                let raw = std::fs::read_to_string(manifest_path)
                    .map_err(|e| format!("读取清单失败: {e}"))?;
                let mut manifest: serde_json::Value =
                    serde_json::from_str(&raw).map_err(|e| format!("清单 JSON 解析失败: {e}"))?;
                let fields = easyssh_core::ota_sign::parse_manifest_fields(&manifest)?;
                let sig = easyssh_core::ota_sign::sign_manifest(&fields, &seed)?;
                manifest
                    .as_object_mut()
                    .ok_or("清单根必须是 JSON 对象")?
                    .insert(
                        easyssh_core::ota_sign::OTA_SIG_FIELD.to_string(),
                        serde_json::Value::String(sig),
                    );
                let pretty = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
                if let Some(i) = args.iter().position(|a| a == "--out") {
                    let out = args.get(i + 1).ok_or("--out 需要输出文件路径")?;
                    std::fs::write(out, pretty).map_err(|e| format!("写入输出失败: {e}"))?;
                    eprintln!("已签名清单写入: {out}");
                } else {
                    let mut so = std::io::stdout();
                    let _ = so.write_all(pretty.as_bytes());
                    let _ = so.write_all(b"\n");
                }
                Ok(())
            }
            _ => Err("用法:\n  essh-sign --keygen <私钥文件>\n  essh-sign sign <清单.json> <私钥文件> [--out 输出.json]".into()),
        }
    })();
    if let Err(e) = err {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}
