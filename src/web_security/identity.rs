//! 服务端长期身份密钥（Ed25519）。
//!
//! ## 作用
//!
//! 身份密钥**不参与**任何密钥派生，只做一件事：对握手 Transcript 签名，
//! 让客户端能确认"我确实在和这台服务器握手，而不是中间人"。
//!
//! 这与"会话加密"是两件事：
//!
//! - 身份密钥回答**"你是谁"**（长期、可固定、可轮换、可撤销）；
//! - 临时 X25519 密钥回答**"本次会话的密钥是什么"**（每次握手重新生成，
//!   提供前向保密）。
//!
//! 混用两者（用身份私钥直接加密业务数据）会让前向保密消失，因此代码层面
//! 严格分离：`ServerIdentity` 只暴露 `sign`，不暴露任何派生子密钥的接口。
//!
//! ## 存储
//!
//! 身份密钥以 32 字节种子形式存放在 `keyring.json`（该文件本身由
//! XChaCha20-Poly1305 + 主密钥加密，见 `crate::crypto_storage`）。
//! 绝不硬编码、绝不写进环境变量、绝不发送到浏览器（只发送公钥）。

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

/// 服务端身份（签名密钥对 + Key ID）。
pub struct ServerIdentity {
    signing: SigningKey,
    kid: String,
}

impl ServerIdentity {
    /// 从 32 字节种子恢复（种子来自 keyring.json）。
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        let kid = key_id_of(&signing.verifying_key().to_bytes());
        Self { signing, kid }
    }

    /// 生成新的身份密钥，同时返回需要持久化的种子。
    #[allow(dead_code)]
    pub fn generate() -> (Self, [u8; 32]) {
        let mut seed = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
        (Self::from_seed(&seed), seed)
    }

    /// Ed25519 公钥（32 字节）。
    pub fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// 身份 Key ID（16 个十六进制字符），与公钥一一对应。
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// 对消息签名。
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// 可公开的身份指纹：`hex(SHA-256(pubkey)[..16])`，共 32 字符。
    ///
    /// 用于**带外**核对（启动横幅 / `taygedo-rs --identity-fingerprint`）。
    /// 客户端首次访问时把该值固定下来，之后每次握手都比对。
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.public_key())
    }
}

/// 公钥指纹。
pub fn fingerprint_of(public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(public_key);
    let mut s = String::with_capacity(32);
    for b in &digest[..16] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 由公钥计算 Key ID。
pub fn key_id_of(public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(public_key);
    let mut s = String::with_capacity(16);
    for b in &digest[..8] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 验签。任何长度/编码错误都返回 `false`（不区分原因，避免给攻击者 oracle）。
///
/// 服务端自身不验签（它只签），该函数供测试与未来的"客户端证书"场景使用。
#[allow(dead_code)]
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    vk.verify(message, &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let (id, seed) = ServerIdentity::generate();
        let msg = b"transcript-bytes";
        let sig = id.sign(msg);
        assert!(verify(&id.public_key(), msg, &sig));
        // 换消息
        assert!(!verify(&id.public_key(), b"other", &sig));
        // 换公钥
        let (other, _) = ServerIdentity::generate();
        assert!(!verify(&other.public_key(), msg, &sig));
        // 篡改签名
        let mut bad = sig;
        bad[0] ^= 0xff;
        assert!(!verify(&id.public_key(), msg, &bad));

        // 种子可复现同一身份
        let restored = ServerIdentity::from_seed(&seed);
        assert_eq!(restored.public_key(), id.public_key());
        assert_eq!(restored.kid(), id.kid());
    }

    #[test]
    fn kid_and_fingerprint_shapes() {
        let id = ServerIdentity::from_seed(&[7u8; 32]);
        assert_eq!(id.kid().len(), 16);
        assert_eq!(id.fingerprint().len(), 32);
        assert!(id.kid().bytes().all(|b| b.is_ascii_hexdigit()));
        // 不同种子必须不同
        let id2 = ServerIdentity::from_seed(&[8u8; 32]);
        assert_ne!(id.kid(), id2.kid());
        assert_ne!(id.fingerprint(), id2.fingerprint());
    }
}
