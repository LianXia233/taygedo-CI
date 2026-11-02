//! 对称密码原语封装：HKDF-SHA256 + XChaCha20-Poly1305。
//!
//! 只是薄封装，不自创任何算法：参数选择全部来自 RFC 5869 / RFC 8439 与
//! libsodium 的 XChaCha20-Poly1305 构造（HChaCha20 派生子密钥 + 96 位 nonce）。

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::protocol::NONCE_LEN;

/// HKDF-SHA256 派生 32 字节子密钥。
pub fn hkdf32(ikm: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .expect("HKDF 输出 32 字节必然合法");
    Zeroizing::new(okm)
}

/// XChaCha20-Poly1305 加密。返回 `密文 || tag`。
pub fn seal(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, ()> {
    let cipher = XChaCha20Poly1305::new_from_slice(&key[..]).map_err(|_| ())?;
    cipher
        .encrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| ())
}

/// 解密。失败一律返回 `Err(())`，调用方负责映射为不泄露细节的错误。
pub fn open(
    key: &[u8; 32],
    nonce: &[u8; NONCE_LEN],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, ()> {
    let cipher = XChaCha20Poly1305::new_from_slice(&key[..]).map_err(|_| ())?;
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| ())
}

/// 生成 24 字节随机 nonce（OS CSPRNG）。
pub fn random_nonce() -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut n);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let key = [1u8; 32];
        let nonce = [2u8; NONCE_LEN];
        let ct = seal(&key, &nonce, b"aad", b"hello").unwrap();
        assert_eq!(
            open(&key, &nonce, b"aad", &ct).unwrap().as_slice(),
            b"hello"
        );
    }

    #[test]
    fn wrong_aad_fails() {
        let key = [1u8; 32];
        let nonce = [2u8; NONCE_LEN];
        let ct = seal(&key, &nonce, b"aad", b"hello").unwrap();
        assert!(open(&key, &nonce, b"other", &ct).is_err());
    }

    #[test]
    fn wrong_nonce_fails() {
        let key = [1u8; 32];
        let ct = seal(&key, &[2u8; NONCE_LEN], b"aad", b"hello").unwrap();
        assert!(open(&key, &[3u8; NONCE_LEN], b"aad", &ct).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let nonce = [2u8; NONCE_LEN];
        let ct = seal(&[1u8; 32], &nonce, b"a", b"hello").unwrap();
        assert!(open(&[9u8; 32], &nonce, b"a", &ct).is_err());
    }

    #[test]
    fn hkdf_is_domain_separated() {
        let a = hkdf32(b"ikm", b"salt", b"one");
        let b = hkdf32(b"ikm", b"salt", b"two");
        let c = hkdf32(b"ikm", b"other", b"one");
        assert_ne!(a[..], b[..]);
        assert_ne!(a[..], c[..]);
        assert_eq!(a[..], hkdf32(b"ikm", b"salt", b"one")[..]);
    }
}
