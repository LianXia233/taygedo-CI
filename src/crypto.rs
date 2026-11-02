//! 密码学原语：凭据加密、应用层会话加密。
//!
//! 包含两部分：
//! 1. **凭据加密**（scrypt + AES-256-GCM）：用于账号密码的静态存储；
//! 2. **应用层会话加密**（X25519 ECDH + HKDF-SHA256 + AES-256-GCM）：
//!    用于 HTTP 之上的请求/响应正文加密，见 `web_security`。
//!
//! 内置 WebUI 已移除，因此不再有"登录口令哈希"这一层：OpenWrt 上鉴权由
//! LuCI/ubus 会话负责，独立部署只接受受信来源（回环/内网白名单）的调用。

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use scrypt::{scrypt, Params};

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
    let ct = cipher
        .encrypt(nonce, password.as_bytes())
        .expect("加密失败");

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
pub fn decrypt_password(
    encrypted: &EncryptedPassword,
    credential_key: &str,
) -> Result<String, String> {
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

    let iv = URL_SAFE_NO_PAD
        .decode(&encrypted.iv)
        .map_err(|_| "iv 解码失败")?;
    let tag = URL_SAFE_NO_PAD
        .decode(&encrypted.tag)
        .map_err(|_| "tag 解码失败")?;
    let data = URL_SAFE_NO_PAD
        .decode(&encrypted.data)
        .map_err(|_| "data 解码失败")?;

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
// 比较与随机
// ---------------------------------------------------------------------------

/// 恒定时间字节比较，避免校验路径的时序侧信道。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
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
