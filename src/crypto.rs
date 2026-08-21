//! 凭据（密码）加密，与上游 `config/credentials.ts` 兼容（v2 scrypt + AES-256-GCM）。

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use aes_gcm::aead::generic_array::GenericArray;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use scrypt::{Params, scrypt};

use crate::models::EncryptedPassword;

/// 生成 32 字节 base64url 凭据密钥。
pub fn generate_credential_key() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn derive_scrypt_key(credential_key: &str, salt: &[u8]) -> Vec<u8> {
    // N=16384, r=8, p=1 → log_n = 14
    let params = Params::new(14, 8, 1, 32).expect("scrypt 参数");
    let mut out = [0u8; 32];
    scrypt(credential_key.as_bytes(), salt, &params, &mut out).expect("scrypt 派生失败");
    out.to_vec()
}

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

fn sha256(data: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).to_vec()
}

/// 计算登录密码哈希：hex(sha256(salt + ":" + password))。
pub fn hash_password(password: &str, salt: &str) -> String {
    let mut buf = Vec::with_capacity(salt.len() + 1 + password.len());
    buf.extend_from_slice(salt.as_bytes());
    buf.push(b':');
    buf.extend_from_slice(password.as_bytes());
    sha256(&buf).iter().map(|b| format!("{:02x}", b)).collect()
}

/// 生成 n 字节随机十六进制字符串。
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}
