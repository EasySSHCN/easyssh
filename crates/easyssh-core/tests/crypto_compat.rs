//! 加密协议兼容性测试：与 EasySSH 3.x (Electron/Node crypto) 生成的向量互通。
//! 向量由 node crypto 生成（3.x 原始实现），Rust 端必须能解密；反向 Rust 加密 →
//! Node 能解密（由 CI/手工用 node 脚本校验）。
use easyssh_core::crypto::{decrypt_secret, encrypt_secret, unwrap_dek, wrap_dek};
use base64::Engine;

const DEK_B64: &str = "SpXviqeqGU5AYQHvMkZKe2SGvhgsekvd/LOTDvaAwfg=";
const ENC_VECTOR: &str =
    "enc:v1:KlOhkL8uopRRvJiI:deWN+UArQRksIOKIyK5qvA==:2DcXhfKdZMq5/VosM+GWREz+DN+X4A==";
const PLAIN: &str = "我的密码MyPass123!";
const PW: &str = "master123";
const WRAP_VECTOR: &str =
    "wrap:v1:2aoXY1q1WXd+ptg6WoanBA==:UZqf4vyjfrJ9oWZq:bBhdkYla4/bAZCxRZfsatA==:rf4Eo6+RqbVLQlGIbfmxdGN3gIScEWIfn2tSsqLfVgI=";

fn dek() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(DEK_B64)
        .expect("decode dek")
}

#[test]
fn decrypt_node_encrypted_secret() {
    let pt = decrypt_secret(ENC_VECTOR, &dek()).expect("decrypt");
    assert_eq!(pt, PLAIN);
}

#[test]
fn encrypt_roundtrip_matches_node_format() {
    let enc = encrypt_secret(PLAIN, &dek()).expect("encrypt");
    assert!(enc.starts_with("enc:v1:"));
    // 结构：三段 base64（iv 12B / tag 16B / 密文=明文长度）
    let body = enc["enc:v1:".len()..].to_string();
    let parts: Vec<&str> = body.split(':').collect();
    assert_eq!(parts.len(), 3);
    let dec = base64::engine::general_purpose::STANDARD;
    assert_eq!(dec.decode(parts[0]).unwrap().len(), 12);
    assert_eq!(dec.decode(parts[1]).unwrap().len(), 16);
    assert_eq!(dec.decode(parts[2]).unwrap().len(), PLAIN.len());
    // 自解密回原文
    assert_eq!(decrypt_secret(&enc, &dek()).unwrap(), PLAIN);
}

#[test]
fn unwrap_node_wrapped_dek() {
    let got = unwrap_dek(WRAP_VECTOR, PW).expect("unwrap");
    assert_eq!(got, dek());
}

#[test]
fn unwrap_wrong_password_fails() {
    assert!(unwrap_dek(WRAP_VECTOR, "wrong").is_err());
}

#[test]
fn wrap_roundtrip() {
    // v4.3.1：新包裹为 wrap:v3(Argon2id)；v1 向量兼容由 unwrap_node_wrapped_dek 覆盖，
    // 服务器版 v4.3.0 的 wrap:v2(600k) 兼容由 lib 内 wrap_v2_compat 测试覆盖
    let wrapped = wrap_dek(&dek(), PW).expect("wrap");
    assert!(wrapped.starts_with("wrap:v3:"));
    assert_eq!(unwrap_dek(&wrapped, PW).unwrap(), dek());
    assert!(unwrap_dek(&wrapped, "nope").is_err());
}

#[test]
fn encrypt_empty_and_passthrough() {
    assert_eq!(encrypt_secret("", &dek()).unwrap(), "");
    assert_eq!(encrypt_secret(ENC_VECTOR, &dek()).unwrap(), ENC_VECTOR);
    assert_eq!(decrypt_secret("plain旧明文", &dek()).unwrap(), "plain旧明文");
}
