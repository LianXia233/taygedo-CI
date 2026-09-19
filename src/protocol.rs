//! 签名 / 加密 / 请求构造 辅助函数，与上游 Node 实现一一对应。

use std::collections::BTreeMap;

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::Rng;
use url::form_urlencoded;

use crate::constants::*;

/// MD5 十六进制摘要
pub fn md5_hex(s: &str) -> String {
    format!("{:x}", md5::compute(s.as_bytes()))
}

/// 老虎 / 云平台签名：按 key 字典序拼接 value，再拼 secret，取 MD5。
pub fn sign(data: &BTreeMap<String, String>, secret: &str) -> String {
    let mut joined = String::new();
    for (_k, v) in data.iter() {
        joined.push_str(v);
    }
    joined.push_str(secret);
    md5_hex(&joined)
}

/// AES-128-ECB + PKCS7 + Base64 编码（key 取 secret 末 16 字节）。
pub fn aes_ecb_base64(value: &str, secret: &str) -> String {
    let key = &secret.as_bytes()[secret.len() - 16..];
    let mut data = value.as_bytes().to_vec();
    let pad = 16 - data.len() % 16;
    data.extend(std::iter::repeat_n(pad as u8, pad));

    let cipher = Aes128::new_from_slice(key).expect("16 字节 key");
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    STANDARD.encode(out)
}

/// 生成 8 位随机 nonce（字母表 62 字符，公平取模）。
const NONCE_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

pub fn make_nonce() -> String {
    let mut rng = rand::thread_rng();
    let mut nonce = String::new();
    let fair_range = (256 / NONCE_ALPHABET.len()) * NONCE_ALPHABET.len();
    while nonce.len() < 8 {
        let b: u8 = rng.gen();
        if (b as usize) >= fair_range {
            continue;
        }
        nonce.push(NONCE_ALPHABET[b as usize % NONCE_ALPHABET.len()] as char);
    }
    nonce
}

/// ds 签名：`{ts},{nonce},{md5(ts+nonce+appVer+secret)}`
pub fn make_ds(timestamp: u64, nonce: &str, app_version: &str) -> String {
    let signature = md5_hex(&format!("{}{}{}{}", timestamp, nonce, app_version, TAYGEDO_DS_SECRET));
    format!("{},{},{}", timestamp, nonce, signature)
}

/// 老虎 Android 基础参数。
/// `version_field` 为 `versionCode` 时带 `imei`/`versionCode`；否则带 `mac`/`version`。
pub fn laohu_base_params(device_id: &str, t: &str, version_field: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("adm".into(), String::new());
    m.insert("appId".into(), LAOHU_APP_ID.into());
    m.insert("bid".into(), "com.pwrd.htassistant".into());
    m.insert("channelId".into(), LAOHU_CHANNEL_ID.into());
    m.insert("deviceId".into(), device_id.into());
    m.insert("deviceModel".into(), LAOHU_DEVICE_MODEL.into());
    m.insert("deviceName".into(), LAOHU_DEVICE_MODEL.into());
    m.insert("deviceSys".into(), LAOHU_DEVICE_SYS.into());
    m.insert("deviceType".into(), LAOHU_DEVICE_MODEL.into());
    m.insert("idfa".into(), String::new());
    m.insert("sdkVersion".into(), LAOHU_SDK_VERSION.into());
    m.insert("t".into(), t.into());
    if version_field == "versionCode" {
        m.insert("imei".into(), String::new());
        m.insert("versionCode".into(), LAOHU_VERSION_CODE.into());
    } else {
        m.insert("mac".into(), String::new());
        m.insert("version".into(), LAOHU_VERSION_CODE.into());
    }
    m
}

/// 生成带签名的表单体。
/// `include_empty` 为 true 时保留空值字段，否则剔除空值（与上游 formEncodeNonEmpty 一致）。
pub fn signed_body(mut data: BTreeMap<String, String>, secret: &str, include_empty: bool) -> String {
    let sign = sign(&data, secret);
    data.insert("sign".into(), sign);
    form_encode(&data, include_empty)
}

/// 表单编码（key/value 均做百分号编码）。
pub fn form_encode(data: &BTreeMap<String, String>, include_empty: bool) -> String {
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for (k, v) in data.iter() {
        if !include_empty && v.is_empty() {
            continue;
        }
        ser.append_pair(k, v);
    }
    ser.finish()
}

/// 通用表单编码（用于无签名场景，如用户中心登录）。
pub fn form_encode_pairs<'a, I>(pairs: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        ser.append_pair(k, v);
    }
    ser.finish()
}

/// 生成 32 位十六进制 deviceId（等价 Node `randomBytes(16).toString('hex')`）。
pub fn random_hex_32() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 当前秒级时间戳
pub fn now_seconds() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// 当前毫秒级时间戳
pub fn now_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

// ---------------------------------------------------------------------------
// 应用层会话加密原语（X25519 + HKDF-SHA256）
// ---------------------------------------------------------------------------

/// 生成 X25519 私钥（32 字节密码学随机，clamping 由 dalek 内部完成）。
pub fn x25519_generate_secret() -> [u8; 32] {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// 由私钥推导公钥。
pub fn x25519_public_from_secret(secret: &[u8; 32]) -> [u8; 32] {
    use x25519_dalek::{PublicKey, StaticSecret};
    let sk = StaticSecret::from(*secret);
    PublicKey::from(&sk).to_bytes()
}

/// 生成 32 字节随机数据（X25519 私钥等）。
///
/// 预留：供需要独立随机源的新协议扩展使用；当前握手路径直接用
/// `x25519_generate_secret()`，session 层用 `crypto::random_hex` / `random_bytes`。
#[allow(dead_code)]
pub fn random_32() -> [u8; 32] {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// X25519 ECDH：返回共享密钥。
///
/// 结果为全零时返回 `None`——全零共享密钥意味着对端提交了低阶点，
/// 属主动攻击特征，必须拒绝而非静默使用。
pub fn x25519_diffie_hellman(
    our_secret: &[u8; 32],
    peer_public: &[u8; 32],
) -> Option<[u8; 32]> {
    use x25519_dalek::{PublicKey, SharedSecret, StaticSecret};
    let sk = StaticSecret::from(*our_secret);
    let pk = PublicKey::from(*peer_public);
    let shared: SharedSecret = sk.diffie_hellman(&pk);
    if shared.as_bytes().iter().all(|b| *b == 0) {
        None
    } else {
        Some(shared.to_bytes())
    }
}

/// HKDF-SHA256 派生指定长度的密钥。
///
/// `salt` 必须包含双方公钥（提供会话绑定性，抵御未知密钥共享攻击），
/// `info` 用作域分隔，保证同一共享密钥派生出不同用途的子密钥。
pub fn hkdf_sha256(
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    out_len: usize,
) -> Vec<u8> {
    use hkdf::Hkdf;
    use sha2::Sha256;
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = vec![0u8; out_len];
    hk.expand(info, &mut out).expect("HKDF 输出长度合法");
    out
}

/// 恒定时间比较（转发，便于 protocol 层直接使用）。
///
/// 预留：供后续在 protocol 层做密钥/摘要比对时复用；
/// 当前所有比较都直接走 `crypto::constant_time_eq`。
#[allow(dead_code)]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    crate::crypto::constant_time_eq(a, b)
}

