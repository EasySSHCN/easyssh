//! 用真实线上信封 + 真实主密钥验证 Rust 信封打开逻辑（与 3.x/官网协议一致）。
use base64::Engine as _;

const ENV_JSON: &str = include_str!("env.json");

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::Mac;
    let mut hm = hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("hmac key");
    hm.update(data);
    hm.finalize().into_bytes().to_vec()
}

fn hkdf_sha256(master: &[u8], salt: &str, info: &str, len: usize) -> Vec<u8> {
    use hmac::Mac;
    // RFC 5869 Extract: PRK = HMAC-Hash(salt, IKM) —— salt 为 HMAC 键，IKM(master) 为数据
    let mut prk_h = hmac::Hmac::<sha2::Sha256>::new_from_slice(salt.as_bytes()).expect("hmac key");
    prk_h.update(master);
    let prk = prk_h.finalize().into_bytes();
    let info_b = info.as_bytes();
    let mut out = Vec::with_capacity(len);
    let mut prev: Vec<u8> = Vec::new();
    let mut counter: u8 = 1;
    while out.len() < len {
        let mut hm = hmac::Hmac::<sha2::Sha256>::new_from_slice(&prk).expect("hmac key");
        hm.update(&prev);
        hm.update(info_b);
        hm.update(&[counter]);
        let block = hm.finalize().into_bytes();
        let take = (len - out.len()).min(block.len());
        out.extend_from_slice(&block[..take]);
        prev = block.to_vec();
        counter += 1;
    }
    out
}

#[test]
fn open_real_envelope() {
    let env: serde_json::Value = serde_json::from_str(ENV_JSON).expect("parse env");
    // 主密钥夹具仅在发布服务器上存在；缺失时跳过（本机/公开环境不参与真实信封验证）
    let master_path = if cfg!(windows) { r"C:\tmp\master-test.json" } else { "/tmp/master-test.json" };
    let Ok(master_b64) = std::fs::read_to_string(master_path) else {
        eprintln!("skip open_real_envelope: master fixture {} 不存在", master_path);
        return;
    };
    let mv: serde_json::Value = serde_json::from_str(&master_b64).expect("parse master");
    let master = base64::engine::general_purpose::STANDARD
        .decode(mv.get("masterKeyBase64").unwrap().as_str().unwrap())
        .expect("decode master");

    let ts = env.get("ts").and_then(|x| x.as_i64()).unwrap();
    let nonce = env.get("nonce").and_then(|x| x.as_str()).unwrap();
    let ct = env.get("ct").and_then(|x| x.as_str()).unwrap();
    let tag = env.get("tag").and_then(|x| x.as_str()).unwrap();
    let mac = env.get("mac").and_then(|x| x.as_str()).unwrap();

    let enc_key = hkdf_sha256(&master, "easyssh-update-check-v1", "aes-256-gcm", 32);
    let mac_key = hkdf_sha256(&master, "easyssh-update-check-v1", "hmac-sha256", 32);

    let mac_data = format!("{ts}.{nonce}.{ct}.{tag}");
    let want = hmac_sha256(&mac_key, mac_data.as_bytes());
    let got = base64::engine::general_purpose::STANDARD.decode(mac).expect("decode mac");
    assert_eq!(want, got, "HMAC 校验不一致: want={:?} got={:?}", want, got);

    use aes_gcm::aead::{Aead, KeyInit, Payload};
    let a = aes_gcm::Aes256Gcm::new_from_slice(&enc_key).unwrap();
    let mut full = base64::engine::general_purpose::STANDARD.decode(ct).unwrap();
    let tag_b = base64::engine::general_purpose::STANDARD.decode(tag).unwrap();
    full.extend_from_slice(&tag_b);
    let pt = a
        .decrypt(
            aes_gcm::Nonce::from_slice(&base64::engine::general_purpose::STANDARD.decode(nonce).unwrap()),
            Payload { msg: full.as_ref(), aad: b"" },
        )
        .expect("gcm decrypt");
    let m: serde_json::Value = serde_json::from_slice(&pt).unwrap();
    println!("manifest latestVersion: {}", m.get("latestVersion").unwrap());
}
