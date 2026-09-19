//! 密码学原语：口令哈希、凭据加密、应用层会话加密。
//!
//! 包含三部分：
//! 1. **口令哈希**（v2 scrypt）：用于 WebUI 登录口令，抗离线暴力破解；
//! 2. **凭据加密**（scrypt + AES-256-GCM）：用于账号密码的静态存储；
//! 3. **应用层会话加密**（X25519 ECDH + HKDF-SHA256 + AES-256-GCM）：
//!    用于 HTTP 之上的请求/响应正文加密，见 `session.rs`。
//!
//! v1 口令哈希（单轮 sha256）仅用于向后兼容校验，新密码一律按 v2 写入。

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use aes_gcm::aead::generic_array::GenericArray;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{Rng, RngCore};
use scrypt::{Params, scrypt};

use crate::models::EncryptedPassword;

/// 生成 32 字节 base64url 凭据密钥。
pub fn generate_credential_key() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// scrypt 参数：N=16384, r=8, p=1 → log_n = 14，输出 32 字节。
///
/// 该参数在路由器（MT7981 等 ARM Cortex-A53）上单次派生耗时约 50-150ms，
/// 对登录接口可接受，对离线爆破则显著抬高成本。
fn scrypt_params() -> Params {
    Params::new(14, 8, 1, 32).expect("scrypt 参数合法")
}

fn derive_scrypt_key(credential_key: &str, salt: &[u8]) -> Vec<u8> {
    let params = scrypt_params();
    let mut out = [0u8; 32];
    scrypt(credential_key.as_bytes(), salt, &params, &mut out).expect("scrypt 派生失败");
    out.to_vec()
}

// ---------------------------------------------------------------------------
// 凭据（账号密码）加密 —— scrypt + AES-256-GCM
// ---------------------------------------------------------------------------

/// 加密密码。
pub fn encrypt_password(password: &str, credential_key: &str) -> EncryptedPassword {
    let mut salt = [0u8; 16];
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut iv);

    let key = derive_scrypt_key(credential_key, &salt);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("32 字节 key");
    let nonce = GenericArray::from_slice(&iv);
    let ct = cipher.encrypt(nonce, password.as_bytes()).expect("加密失败");

    // aes-gcm 的 encrypt 返回 ciphertext||tag（tag 16 字节）
    let (data, tag) = ct.split_at(ct.len() - 16);

    EncryptedPassword {
        v: 2,
        alg: "AES-256-GCM".into(),
        kdf: Some("scrypt".into()),
        salt: Some(URL_SAFE_NO_PAD.encode(salt)),
        iv: URL_SAFE_NO_PAD.encode(iv),
        tag: URL_SAFE_NO_PAD.encode(tag),
        data: URL_SAFE_NO_PAD.encode(data),
    }
}

/// 解密密码。
pub fn decrypt_password(encrypted: &EncryptedPassword, credential_key: &str) -> Result<String, String> {
    if encrypted.alg != "AES-256-GCM" {
        return Err("不支持的加密密码格式".into());
    }
    let key = match encrypted.v {
        1 => {
            // 旧格式：sha256(credentialKey)
            sha256(credential_key.as_bytes())
        }
        _ => {
            let salt = URL_SAFE_NO_PAD
                .decode(encrypted.salt.as_deref().unwrap_or(""))
                .map_err(|_| "salt 解码失败")?;
            derive_scrypt_key(credential_key, &salt)
        }
    };

    let iv = URL_SAFE_NO_PAD.decode(&encrypted.iv).map_err(|_| "iv 解码失败")?;
    let tag = URL_SAFE_NO_PAD.decode(&encrypted.tag).map_err(|_| "tag 解码失败")?;
    let data = URL_SAFE_NO_PAD.decode(&encrypted.data).map_err(|_| "data 解码失败")?;

    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "key 长度错误")?;
    let nonce = GenericArray::from_slice(&iv);
    let mut ct = data;
    ct.extend_from_slice(&tag);
    let plain = cipher
        .decrypt(nonce, ct.as_slice())
        .map_err(|_| "存储密码解密失败，请检查凭据密钥".to_string())?;
    String::from_utf8(plain).map_err(|_| "解密结果非 UTF-8".to_string())
}

// ---------------------------------------------------------------------------
// WebUI 登录口令哈希
// ---------------------------------------------------------------------------

/// 口令哈希版本：v2 = scrypt + 16 字节盐。
pub const PWD_HASH_V2: u8 = 2;
/// 口令哈希版本：v1 = 单轮 sha256 + 8 字节盐（旧格式，仅兼容校验）。
pub const PWD_HASH_V1: u8 = 1;

/// 计算 v2 口令哈希：`hex(scrypt(password, salt, N=16384, r=8, p=1, dkLen=32))`。
///
/// 与 v1 的 `sha256(salt:password)` 相比，单次校验成本提升约 5-6 个数量级，
/// 使离线字典攻击在实用时间尺度上不可行。
pub fn hash_password_v2(password: &str, salt: &str) -> String {
    let params = scrypt_params();
    let mut out = [0u8; 32];
    scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).expect("scrypt 派生失败");
    out.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 计算 v1 口令哈希（仅用于兼容校验与迁移）。
pub fn hash_password(password: &str, salt: &str) -> String {
    let mut buf = Vec::with_capacity(salt.len() + 1 + password.len());
    buf.extend_from_slice(salt.as_bytes());
    buf.push(b':');
    buf.extend_from_slice(password.as_bytes());
    sha256(&buf).iter().map(|b| format!("{:02x}", b)).collect()
}

/// 恒定时间字节比较，避免口令校验的时序侧信道。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn sha256(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).to_vec()
}

/// 生成 n 字节随机十六进制字符串。
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// 生成 len 字节密码学随机数据。
///
/// 预留：凭证密钥、盐值等场景使用；当前由 `generate_credential_key`
/// 等封装函数内部直接调用随机源。
#[allow(dead_code)]
pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

/// 生成可读性优先的随机口令（用于取代默认 admin 口令）。
///
/// 字母表刻意排除易混淆字符（0/O、1/l/I），长度 16，熵约 91 bit。
pub fn generate_passphrase(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    // 公平取模：丢弃超出 256/len 整数倍范围的字节，避免模偏置
    let fair_range = (256 / ALPHABET.len()) * ALPHABET.len();
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let b: u8 = rng.gen();
        if (b as usize) >= fair_range {
            continue;
        }
        out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pwd_hash_v2_is_deterministic_and_salt_sensitive() {
        let h1 = hash_password_v2("s3cret", "saltA");
        let h2 = hash_password_v2("s3cret", "saltA");
        let h3 = hash_password_v2("s3cret", "saltB");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn passphrase_has_expected_shape() {
        let p = generate_passphrase(16);
        assert_eq!(p.len(), 16);
        assert!(p.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(p, generate_passphrase(16));
    }

    #[test]
    fn credential_roundtrip() {
        let key = generate_credential_key();
        let enc = encrypt_password("hunter2", &key);
        assert_eq!(decrypt_password(&enc, &key).unwrap(), "hunter2");
        // 错误密钥必须解密失败（GCM 认证标签校验）
        let other = generate_credential_key();
        assert!(decrypt_password(&enc, &other).is_err());
    }
}
