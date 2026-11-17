/* v5.2.0/F-R10-14：PuTTY .ppk 私钥直读导入
 * - 支持 PPK v2 / v3 两种文件头（PuTTY-User-Key-File-2: / -3:）
 * - 算法支持 ssh-ed25519 / ssh-rsa / ecdsa-sha2-nistp256
 * - v3 未加密：Private-Lines base64 拼接后即 openssh-key-v1 容器原始字节，
 *   直接包上 -----BEGIN OPENSSH PRIVATE KEY----- 交给 russh_keys::decode_secret_key
 * - v2 未加密：私有块是 PuTTY 自有 wire 编码（无 checkint/comment），
 *   按算法逐字段解析后重新拼装标准 openssh-key-v1 容器，再走同一解析器
 * - 带口令加密（v3 Key-Derivation 非 none / v2 Encryption 非 none）不解密，明确报错
 * - v2 MAC = HMAC-SHA1("putty-private-key-file-mac-v2", 拼接串)
 *   v3 MAC = HMAC-SHA256("putty-private-key-file-mac-v3", 拼接串)
 *   拼接串 = string(算法名) || string(公钥 blob) || string(私有块)
 * - 输出：PKCS#8 PEM 私钥 + OpenSSH 一行公钥（复用 v4 现有管线，russh 直接支持） */
use base64::Engine as _;
use hmac::{Hmac, Mac};
use russh_keys::key::KeyPair;
use russh_keys::PublicKeyBase64;
use sha2::Sha256;
type HmacSha256 = Hmac<Sha256>;
type HmacSha1 = Hmac<sha1::Sha1>;

const ERR_ENCRYPTED: &str = "该 .ppk 带口令加密，请先用 PuTTYgen 打开并去掉密钥口令(或改存为 OpenSSH 格式)后再导入";

/* ---------- PPK 文本结构解析 ---------- */

#[derive(Debug)] // Debug 供测试 unwrap_err 使用
struct PpkFile {
    version: u32,          // 2 或 3
    algo: String,          // 头部声明的算法名（如 ssh-ed25519）
    #[allow(dead_code)]
    comment: String,
    public_blob: Vec<u8>,  // SSH wire 公钥 blob（含算法名长度前缀）
    private_bytes: Vec<u8>, // v2=PuTTY 私有块明文；v3=openssh-key-v1 容器原始字节
    mac: Vec<u8>,
}

/// 解析 PPK 头/公钥块/私钥块/MAC（不校验 MAC）
fn parse_ppk_text(text: &str) -> Result<PpkFile, String> {
    let mut version = 0u32;
    let mut algo = String::new();
    let mut comment = String::new();
    let mut encryption = String::new();
    let mut kdf = String::new();
    let mut mac = String::new();
    let mut public_b64 = String::new();
    let mut private_b64 = String::new();

    let mut lines = text.lines().map(|l| l.trim_end()).peekable();
    // 第一段：头字段（Key: Value），遇到 Public-Lines 结束
    while let Some(line) = lines.next() {
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| "不是有效的 PuTTY .ppk 文件(缺少 Key: Value 头字段)".to_string())?;
        let k = k.trim();
        let v = v.trim();
        match k {
            "PuTTY-User-Key-File-2" => {
                version = 2;
                algo = v.to_string();
            }
            "PuTTY-User-Key-File-3" => {
                version = 3;
                algo = v.to_string();
            }
            "Encryption" => encryption = v.to_string(),
            "Comment" => comment = v.to_string(),
            "Key-Derivation" => kdf = v.to_string(),
            "Public-Lines" => {
                collect_b64_lines(&mut lines, v, &mut public_b64)?;
                break; // 公钥块之后进入第二段
            }
            _ => {} // 未知头字段(如 v3 的 Source/Generation)忽略
        }
    }
    // 第二段：Private-Lines + Private-MAC
    let mut saw_private = false;
    while let Some(line) = lines.next() {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "Private-Lines" => {
                    collect_b64_lines(&mut lines, v, &mut private_b64)?;
                    saw_private = true;
                }
                "Private-MAC" => mac = v.trim().replace(' ', "").to_string(),
                _ => {}
            }
        }
    }
    if version == 0 || algo.is_empty() {
        return Err("不是有效的 PuTTY .ppk 文件(缺少 PuTTY-User-Key-File-2/-3 头)".to_string());
    }
    if !saw_private || public_b64.is_empty() {
        return Err(".ppk 文件不完整(缺少 Public-Lines/Private-Lines 数据块)".to_string());
    }
    if version == 2 && !encryption.eq_ignore_ascii_case("none") {
        return Err(ERR_ENCRYPTED.to_string());
    }
    if version == 3 && (!kdf.eq_ignore_ascii_case("none") || !encryption.eq_ignore_ascii_case("none")) {
        return Err(ERR_ENCRYPTED.to_string());
    }
    let b64e = base64::engine::general_purpose::STANDARD;
    let public_blob = b64e
        .decode(public_b64.trim())
        .map_err(|e| format!(".ppk 公钥块 base64 解码失败: {e}"))?;
    let private_bytes = b64e
        .decode(private_b64.trim())
        .map_err(|e| format!(".ppk 私钥块 base64 解码失败: {e}"))?;
    let mac = hex_decode(&mac).ok_or(".ppk Private-MAC 不是合法 hex")?;
    Ok(PpkFile {
        version,
        algo,
        comment,
        public_blob,
        private_bytes,
        mac,
    })
}

/// 按 "Public-Lines: N" 的 N 收集接下来 N 行 base64
fn collect_b64_lines<I, S>(lines: &mut std::iter::Peekable<I>, n_str: &str, out: &mut String) -> Result<(), String>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let n: usize = n_str
        .trim()
        .parse()
        .map_err(|_| format!("PPK 块行数不是数字: {n_str}"))?;
    for _ in 0..n {
        let l = lines.next().ok_or("PPK 文件在数据块中途结束")?;
        out.push_str(l.as_ref().trim());
    }
    Ok(())
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/* ---------- MAC 校验 ---------- */

fn ppk_string(b: &[u8]) -> Vec<u8> {
    let mut v = (b.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(b);
    v
}

fn mac_concat(ppk: &PpkFile) -> Vec<u8> {
    let mut d = ppk_string(ppk.algo.as_bytes());
    d.extend_from_slice(&ppk_string(&ppk.public_blob));
    d.extend_from_slice(&ppk_string(&ppk.private_bytes));
    d
}

fn verify_mac(ppk: &PpkFile) -> Result<(), String> {
    let expected = match ppk.version {
        2 => {
            let mut m = HmacSha1::new_from_slice(b"putty-private-key-file-mac-v2")
                .map_err(|e| format!("MAC 初始化失败: {e}"))?;
            m.update(&mac_concat(ppk));
            m.finalize().into_bytes().to_vec()
        }
        3 => {
            let mut m = HmacSha256::new_from_slice(b"putty-private-key-file-mac-v3")
                .map_err(|e| format!("MAC 初始化失败: {e}"))?;
            m.update(&mac_concat(ppk));
            m.finalize().into_bytes().to_vec()
        }
        v => return Err(format!("不支持的 .ppk 版本: {v}(仅支持 v2/v3)")),
    };
    if expected != ppk.mac {
        return Err(".ppk Private-MAC 校验失败(文件可能损坏或被篡改)".to_string());
    }
    Ok(())
}

/* ---------- SSH wire 编解码工具 ---------- */

fn read_ssh_string(buf: &[u8], pos: &mut usize) -> Result<Vec<u8>, String> {
    if *pos + 4 > buf.len() {
        return Err(".ppk 私有块格式错误(长度字段越界)".to_string());
    }
    let len = u32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]) as usize;
    *pos += 4;
    if *pos + len > buf.len() {
        return Err(".ppk 私有块格式错误(数据越界)".to_string());
    }
    let s = buf[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(s)
}

fn mpint(bytes: &[u8]) -> Vec<u8> {
    // 去前导零，最高位为 1 时补 0x00 符号位（OpenSSH mpint 规范）
    let mut i = 0;
    while i < bytes.len() && bytes[i] == 0 {
        i += 1;
    }
    let m = &bytes[i..];
    let mut v = if m.first().map(|b| b & 0x80 != 0).unwrap_or(false) {
        vec![0u8]
    } else {
        Vec::new()
    };
    v.extend_from_slice(m);
    ppk_string(&v)
}

/* ---------- openssh-key-v1 容器拼装（供 v2 与 v3 body-only 兜底使用） ---------- */

fn openssh_container(public_blob: &[u8], body: &[u8]) -> Vec<u8> {
    let mut c = b"openssh-key-v1\x00".to_vec();
    c.extend_from_slice(&ppk_string(b"none")); // ciphername
    c.extend_from_slice(&ppk_string(b"none")); // kdfname
    c.extend_from_slice(&ppk_string(b"")); // kdfoptions
    c.extend_from_slice(&1u32.to_be_bytes()); // 1 key
    c.extend_from_slice(&ppk_string(public_blob));
    c.extend_from_slice(&ppk_string(body));
    c
}

fn openssh_body(checkint: u32, algo: &[u8], mut fields: Vec<Vec<u8>>, comment: &str) -> Vec<u8> {
    let mut b = Vec::new();
    // checkint 为裸 u32（非 string 编码），两个值相同用于解密一致性校验
    b.extend_from_slice(&checkint.to_be_bytes());
    b.extend_from_slice(&checkint.to_be_bytes());
    b.extend_from_slice(&ppk_string(algo));
    for f in fields.drain(..) {
        b.extend_from_slice(&f);
    }
    b.extend_from_slice(&ppk_string(comment.as_bytes()));
    // 填充到 8 字节块，填充值为 1..n（openssh-key-v1 规范）
    let pad = 8 - (b.len() % 8);
    for i in 1..=pad as u8 {
        b.push(i);
    }
    b
}

/// 从 PPK 公钥 blob 提取字段（跳过算法名字符串，返回其余 string 字段）
fn blob_fields(public_blob: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut pos = 0usize;
    let name = read_ssh_string(public_blob, &mut pos)?;
    let mut fields = vec![name];
    while pos < public_blob.len() {
        fields.push(read_ssh_string(public_blob, &mut pos)?);
    }
    Ok(fields)
}

/// v2 私有块 PuTTY wire 编码 → 标准 openssh-key-v1 容器字节
fn v2_private_to_container(ppk: &PpkFile) -> Result<Vec<u8>, String> {
    let mut checkint = [0u8; 4];
    rand::Rng::fill(&mut rand::thread_rng(), &mut checkint);
    let ci = u32::from_be_bytes(checkint);
    let pubf = blob_fields(&ppk.public_blob)?;
    match ppk.algo.as_str() {
        "ssh-ed25519" => {
            // v2 私有块：[len][32B seed]
            let mut pos = 0usize;
            let seed = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            if pos != ppk.private_bytes.len() {
                return Err(".ppk ed25519 私有块含多余数据".to_string());
            }
            if seed.len() != 32 {
                return Err(format!(".ppk ed25519 seed 长度异常: {}", seed.len()));
            }
            // 公钥 blob: [name][pk 32B]
            let pk = pubf.get(1).ok_or(".ppk ed25519 公钥 blob 缺字段")?.clone();
            let mut sk = seed.clone();
            sk.extend_from_slice(&pk);
            let fields = vec![ppk_string(&pk), ppk_string(&sk)];
            Ok(openssh_container(&ppk.public_blob, &openssh_body(ci, b"ssh-ed25519", fields, "")))
        }
        "ssh-rsa" => {
            // v2 私有块：[len][e][len][d][len][p][len][q][len][iqmp]
            let mut pos = 0usize;
            let e = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            let d = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            let p = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            let q = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            let iqmp = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            if pos != ppk.private_bytes.len() {
                return Err(".ppk rsa 私有块含多余数据".to_string());
            }
            // 公钥 blob: [name][e][n]
            if pubf.len() != 3 {
                return Err(".ppk rsa 公钥 blob 字段数异常".to_string());
            }
            let fields = vec![
                mpint(&pubf[2]), // n
                mpint(&e),
                mpint(&d),
                mpint(&iqmp),
                mpint(&p),
                mpint(&q),
            ];
            Ok(openssh_container(&ppk.public_blob, &openssh_body(ci, b"ssh-rsa", fields, "")))
        }
        "ecdsa-sha2-nistp256" => {
            // v2 私有块：[len][32B scalar]
            let mut pos = 0usize;
            let scalar = read_ssh_string(&ppk.private_bytes, &mut pos)?;
            if pos != ppk.private_bytes.len() {
                return Err(".ppk ecdsa 私有块含多余数据".to_string());
            }
            // 公钥 blob: [name][curve][Q]
            if pubf.len() != 3 {
                return Err(".ppk ecdsa 公钥 blob 字段数异常".to_string());
            }
            let fields = vec![
                ppk_string(&pubf[1]), // curve 名
                ppk_string(&pubf[2]), // Q
                mpint(&scalar),
            ];
            Ok(openssh_container(
                &ppk.public_blob,
                &openssh_body(ci, b"ecdsa-sha2-nistp256", fields, ""),
            ))
        }
        other => Err(format!("不支持的 .ppk 算法: {other}(仅支持 ed25519/rsa/ecdsa-nistp256)")),
    }
}

/* ---------- 解析入口 ---------- */

fn wrap_openssh_pem(container: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(container);
    let mut pem = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        pem.push('\n');
    }
    pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    pem
}

/// PPK 文本 → russh KeyPair（内部校验 MAC 与结构一致性）
pub fn parse_to_keypair(ppk_text: &str) -> Result<(KeyPair, String, String), String> {
    let ppk = parse_ppk_text(ppk_text)?;
    verify_mac(&ppk)?;
    // v3：私有块应为完整 openssh-key-v1 容器；若不是(保守兜底)，按 v2 风格 body 重组
    let container = if ppk.version == 3 && ppk.private_bytes.starts_with(b"openssh-key-v1\x00") {
        ppk.private_bytes.clone()
    } else {
        v2_private_to_container(&ppk)?
    };
    let pem = wrap_openssh_pem(&container);
    let kp = russh_keys::decode_secret_key(&pem, None)
        .map_err(|e| format!(".ppk 私钥块解析失败: {e}"))?;
    // 公钥一致性：头部声明的算法与解析出的私钥算法必须一致
    let name = kp.name().to_string();
    if !ppk.algo.starts_with(&name) && !name.starts_with(ppk.algo.as_str()) {
        return Err(format!(
            ".ppk 头部算法({})与私钥实际算法({})不一致",
            ppk.algo, name
        ));
    }
    let comment = ppk.comment.clone();
    Ok((kp, name, comment))
}

/// 导入：读文件 → 解析转换 → 写出 PKCS#8 私钥 + 公钥到 <base>/keys/
pub fn import(base_dir: &std::path::Path, ppk_path: &str, _passphrase: Option<&str>) -> Result<serde_json::Value, String> {
    // 注意 v5.2.0/F-R10-14：本实现不支持解密带口令的 .ppk（parse 阶段即拒绝），
    // passphrase 参数仅保留给未来扩展，传入也会走同一条不支持解密错误路径。
    let text = std::fs::read_to_string(ppk_path)
        .map_err(|e| format!("读取 .ppk 文件失败({ppk_path}): {e}"))?;
    let (kp, algo, comment) = parse_to_keypair(&text)?;
    let dir = base_dir.join("keys");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 keys 目录失败: {e}"))?;
    let stem = std::path::Path::new(ppk_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "ppk".into());
    // 文件名安全：仅保留文件名中常规字符
    let stem: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let priv_path = dir.join(format!("imported_{stem}_{ts}.pem"));
    let pub_path = dir.join(format!("imported_{stem}_{ts}.pub"));

    let mut pem = Vec::new();
    russh_keys::encode_pkcs8_pem(&kp, &mut pem).map_err(|e| format!("编码私钥失败: {e}"))?;
    std::fs::write(&priv_path, &pem).map_err(|e| format!("写入私钥失败: {e}"))?;

    let pk = kp.clone_public_key().map_err(|e| format!("提取公钥失败: {e}"))?;
    let pub_line = format!(
        "{} {} {}",
        pk.name(),
        pk.public_key_base64(),
        if comment.is_empty() {
            format!("imported-from-ppk-{ts}")
        } else {
            comment
        }
    );
    std::fs::write(&pub_path, format!("{pub_line}\n")).map_err(|e| format!("写入公钥失败: {e}"))?;

    Ok(serde_json::json!({
        "privPath": priv_path.to_string_lossy(),
        "pubPath": pub_path.to_string_lossy(),
        "pubLine": pub_line,
        "algo": algo,
    }))
}

/* ---------- 测试 ---------- */

#[cfg(test)]
mod tests {
    use super::*;

    /// 从 russh 生成的 KeyPair 构造标准 openssh-key-v1 容器（测试样例用）
    fn keypair_to_container(kp: &KeyPair) -> (Vec<u8>, Vec<u8>) {
        let pub_blob = kp.public_key_bytes();
        let ci = 0x12345678u32;
        let body = match kp {
            KeyPair::Ed25519(sk) => {
                let seed = sk.to_bytes();
                let pk = sk.verifying_key().as_bytes().to_vec();
                let mut sk64 = seed.to_vec();
                sk64.extend_from_slice(&pk);
                let fields = vec![ppk_string(&pk), ppk_string(&sk64)];
                openssh_body(ci, b"ssh-ed25519", fields, "test-comment")
            }
            _ => panic!("测试样例仅构造 ed25519"),
        };
        (openssh_container(&pub_blob, &body), pub_blob)
    }

    fn build_ppk_v3(container: &[u8], pub_blob: &[u8]) -> String {
        let b64e = base64::engine::general_purpose::STANDARD;
        let mut m = HmacSha256::new_from_slice(b"putty-private-key-file-mac-v3").unwrap();
        m.update(&{
            let mut d = ppk_string(b"ssh-ed25519");
            d.extend_from_slice(&ppk_string(pub_blob));
            d.extend_from_slice(&ppk_string(container));
            d
        });
        let mac = hex::encode_lower(&m.finalize().into_bytes());
        let container_b64 = b64e.encode(container);
        let pub_b64 = b64e.encode(pub_blob);
        let pl = (container_b64.len() + 63) / 64;
        format!(
            "PuTTY-User-Key-File-3: ssh-ed25519\r\nKey-Derivation: none\r\nEncryption: none\r\nComment: test-comment\r\nPublic-Lines: {}\r\n{}\r\nPrivate-Lines: {}\r\n{}\r\nPrivate-MAC: {}\r\n",
            (pub_b64.len() + 63) / 64,
            wrap64(&pub_b64),
            pl,
            wrap64(&container_b64),
            mac
        )
    }

    fn wrap64(s: &str) -> String {
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && i % 64 == 0 {
                out.push_str("\r\n");
            }
            out.push(c);
        }
        out
    }

    /// hex 编码小工具（避免引 hex crate）
    mod hex {
        pub fn encode_lower(b: &[u8]) -> String {
            b.iter().map(|x| format!("{x:02x}")).collect()
        }
    }

    #[test]
    fn test_v3_unencrypted_ed25519_roundtrip() {
        let kp = KeyPair::generate_ed25519().expect("生成密钥失败");
        let (container, pub_blob) = keypair_to_container(&kp);
        let ppk_text = build_ppk_v3(&container, &pub_blob);
        let (parsed, algo, comment) = parse_to_keypair(&ppk_text).expect("v3 解析失败");
        assert_eq!(algo, "ssh-ed25519");
        assert_eq!(comment, "test-comment");
        // pk 比对
        let want = match &kp {
            KeyPair::Ed25519(sk) => sk.verifying_key().as_bytes().to_vec(),
            _ => unreachable!(),
        };
        let got = match &parsed {
            KeyPair::Ed25519(sk) => sk.verifying_key().as_bytes().to_vec(),
            _ => panic!("期望 Ed25519"),
        };
        assert_eq!(want, got);
        // encode_pkcs8_pem 写出后 decode_secret_key 再读回
        let mut pem = Vec::new();
        russh_keys::encode_pkcs8_pem(&parsed, &mut pem).expect("PKCS#8 编码失败");
        let reparsed = russh_keys::decode_secret_key(&String::from_utf8(pem).unwrap(), None)
            .expect("PKCS#8 回读失败");
        let got2 = match &reparsed {
            KeyPair::Ed25519(sk) => sk.verifying_key().as_bytes().to_vec(),
            _ => panic!("回读期望 Ed25519"),
        };
        assert_eq!(want, got2);
    }

    #[test]
    fn test_v2_unencrypted_ed25519() {
        // 手工构造最小 v2 ed25519 PPK：私有块 = [len][32B seed]
        let kp = KeyPair::generate_ed25519().expect("生成密钥失败");
        let (seed, pub_blob) = match &kp {
            KeyPair::Ed25519(sk) => (sk.to_bytes().to_vec(), kp.public_key_bytes()),
            _ => unreachable!(),
        };
        // v2 私有块：[len][32B seed]
        let priv_block = ppk_string(&seed);
        let ppk = PpkFile {
            version: 2,
            algo: "ssh-ed25519".into(),
            comment: String::new(),
            public_blob: pub_blob.clone(),
            private_bytes: priv_block,
            mac: Vec::new(),
        };
        let container = v2_private_to_container(&ppk).expect("v2 容器重组失败");
        let pem = wrap_openssh_pem(&container);
        let parsed = russh_keys::decode_secret_key(&pem, None).expect("v2 容器解析失败");
        let got = parsed.public_key_bytes();
        assert_eq!(pub_blob, got);
    }

    #[test]
    fn test_v2_encrypted_rejected() {
        let text = "PuTTY-User-Key-File-2: ssh-ed25519\r\nEncryption: aes256-cbc\r\nComment: x\r\nPublic-Lines: 2\r\nAAAA\r\nAAAA\r\nPrivate-Lines: 1\r\nAAAA\r\nPrivate-MAC: 0000000000000000000000000000000000000000\r\n";
        let err = parse_ppk_text(text).unwrap_err();
        assert!(err.contains("带口令加密"), "错误信息应提示加密: {err}");
    }

    #[test]
    fn test_v3_encrypted_rejected() {
        let text = "PuTTY-User-Key-File-3: ssh-ed25519\r\nKey-Derivation: Argon2id\r\nEncryption: aes256-cbc\r\nComment: x\r\nPublic-Lines: 1\r\nAAAA\r\nPrivate-Lines: 1\r\nAAAA\r\nPrivate-MAC: 00\r\n";
        let err = parse_ppk_text(text).unwrap_err();
        assert!(err.contains("带口令加密"), "错误信息应提示加密: {err}");
    }

    #[test]
    fn test_bad_header_rejected() {
        let err = parse_ppk_text("not a ppk file").unwrap_err();
        assert!(err.contains("不是有效的 PuTTY"), "错误信息应提示无效文件: {err}");
        let err2 = parse_ppk_text("some random text\nwithout ppk header").unwrap_err();
        assert!(err2.contains("不是有效的 PuTTY"), "错误信息应提示无效文件: {err2}");
    }

    #[test]
    fn test_v2_bad_mac_rejected() {
        // 用合法 v3 样本改成 v2 版本头 + 错误 MAC
        let kp = KeyPair::generate_ed25519().expect("生成密钥失败");
        let (container, pub_blob) = keypair_to_container(&kp);
        let b64e = base64::engine::general_purpose::STANDARD;
        let container_b64 = b64e.encode(&container);
        let pub_b64 = b64e.encode(&pub_blob);
        let text = format!(
            "PuTTY-User-Key-File-2: ssh-ed25519\r\nEncryption: none\r\nComment: c\r\nPublic-Lines: 1\r\n{}\r\nPrivate-Lines: 1\r\n{}\r\nPrivate-MAC: deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\r\n",
            pub_b64, container_b64
        );
        let ppk = parse_ppk_text(&text).expect("文本解析失败");
        assert!(verify_mac(&ppk).is_err(), "错误 MAC 必须被拒绝");
    }
}
