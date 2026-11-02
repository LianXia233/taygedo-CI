//! 统一密文文件格式（Envelope）。
//!
//! ## 线格式（全部为大端）
//!
//! ```text
//! offset  len  field
//! 0       8    magic           = "TGDENV01"
//! 8       1    version         = 1
//! 9       1    alg             = 1  (XChaCha20-Poly1305 + HKDF-SHA256)
//! 10      16   key_id          主密钥标识（ASCII 十六进制，非机密）
//! 26      32   salt            每文件随机（HKDF salt）
//! 58      24   nonce           每条消息随机（XChaCha20 24 字节 nonce）
//! 82      4    ct_len          密文长度（含 16 字节 Poly1305 tag）
//! 86      n    ciphertext      ct_len 字节
//! ```
//!
//! ## 严格解析
//!
//! - `magic` 不匹配 → `BadMagic`（**不**退回"当作明文 JSON 再试一次"，
//!   否则明文/密文两种格式会互相混淆，攻击者可借此降级）。
//! - `version != 1` → `UnsupportedVersion`。
//! - `alg != 1` → `UnsupportedAlg`。
//! - `ct_len < TAG_LEN` 或 `blob.len() != HEADER_LEN + ct_len` → `Corrupt`
//!   （截断、篡改长度字段、尾随垃圾三种情况一网打尽）。
//! - `ct_len > MAX_CIPHERTEXT_BYTES` → `TooLarge`（读入前就已按总长拦截，
//!   这里是第二道闸）。

use super::{FileIdentity, StorageError};

/// 文件头 Magic。8 字节，便于与 JSON（`{`/`[`/空白）明确区分。
pub const MAGIC: [u8; 8] = *b"TGDENV01";
/// 当前格式版本。
pub const FORMAT_VERSION: u8 = 1;
/// 算法标识：XChaCha20-Poly1305 + HKDF-SHA256。
pub const ALG_XCHACHA20_POLY1305_HKDF_SHA256: u8 = 1;

pub const SALT_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const KEY_ID_LEN: usize = 16;
/// Poly1305 认证标签长度。
pub const TAG_LEN: usize = 16;

/// 头部总长。
pub const HEADER_LEN: usize = 8 + 1 + 1 + KEY_ID_LEN + SALT_LEN + NONCE_LEN + 4;

/// 单个密文文件的绝对上限（8 MiB）。正常配置文件在数百 KB 量级。
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
/// 密文字段上限。
pub const MAX_CIPHERTEXT_BYTES: usize = MAX_FILE_BYTES - HEADER_LEN;

/// 解析后的头部。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub alg: u8,
    pub key_id: [u8; KEY_ID_LEN],
    pub salt: [u8; SALT_LEN],
    pub nonce: [u8; NONCE_LEN],
    pub ct_len: u32,
}

impl Header {
    pub fn serialize(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        let mut o = 0;
        buf[o..o + 8].copy_from_slice(&MAGIC);
        o += 8;
        buf[o] = self.version;
        o += 1;
        buf[o] = self.alg;
        o += 1;
        buf[o..o + KEY_ID_LEN].copy_from_slice(&self.key_id);
        o += KEY_ID_LEN;
        buf[o..o + SALT_LEN].copy_from_slice(&self.salt);
        o += SALT_LEN;
        buf[o..o + NONCE_LEN].copy_from_slice(&self.nonce);
        o += NONCE_LEN;
        buf[o..o + 4].copy_from_slice(&self.ct_len.to_be_bytes());
        buf
    }
}

/// 解析结果：头部 + 密文切片（零拷贝）。
pub struct Envelope<'a> {
    pub header: Header,
    pub ciphertext: &'a [u8],
}

/// 快速判定是否为密文（只看 Magic）。用于迁移路径区分新旧格式。
pub fn is_envelope(bytes: &[u8]) -> bool {
    bytes.len() >= MAGIC.len() && bytes[..MAGIC.len()] == MAGIC
}

/// 严格解析。任何边界不符都返回错误，绝不"尽力而为"。
pub fn parse(blob: &[u8]) -> Result<Envelope<'_>, StorageError> {
    if blob.len() > MAX_FILE_BYTES {
        return Err(StorageError::TooLarge(blob.len()));
    }
    if blob.len() < HEADER_LEN {
        return Err(StorageError::Corrupt(format!(
            "长度 {} 小于头部 {}",
            blob.len(),
            HEADER_LEN
        )));
    }
    if blob[..8] != MAGIC {
        return Err(StorageError::BadMagic);
    }
    let version = blob[8];
    if version != FORMAT_VERSION {
        return Err(StorageError::UnsupportedVersion(version));
    }
    let alg = blob[9];
    if alg != ALG_XCHACHA20_POLY1305_HKDF_SHA256 {
        return Err(StorageError::UnsupportedAlg(alg));
    }
    let mut key_id = [0u8; KEY_ID_LEN];
    key_id.copy_from_slice(&blob[10..26]);
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&blob[26..58]);
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&blob[58..82]);
    let ct_len = u32::from_be_bytes([blob[82], blob[83], blob[84], blob[85]]) as usize;

    if ct_len < TAG_LEN {
        return Err(StorageError::Corrupt(format!(
            "密文长度 {ct_len} 小于认证标签长度 {TAG_LEN}"
        )));
    }
    if ct_len > MAX_CIPHERTEXT_BYTES {
        return Err(StorageError::TooLarge(HEADER_LEN + ct_len));
    }
    let expected_total = HEADER_LEN + ct_len;
    if blob.len() != expected_total {
        return Err(StorageError::Corrupt(format!(
            "长度不符：头部声明 {expected_total}，实际 {}（截断或被追加数据）",
            blob.len()
        )));
    }

    Ok(Envelope {
        header: Header {
            version,
            alg,
            key_id,
            salt,
            nonce,
            ct_len: ct_len as u32,
        },
        ciphertext: &blob[HEADER_LEN..],
    })
}

/// 构造 AEAD 附加认证数据（AAD）。
///
/// 绑定：域分隔标签 + 格式版本 + 算法 + Key ID + 文件逻辑身份。
/// 由此得到的性质：
/// - 不同文件之间**无法**互换密文（身份不同 → AAD 不同 → 认证失败）；
/// - 同一文件被换了主密钥（Key ID 变化）也无法通过认证；
/// - 未来格式升级不会与旧版本混淆。
pub fn build_aad(id: FileIdentity, h: &Header) -> Vec<u8> {
    let name = id.as_str().as_bytes();
    let mut aad = Vec::with_capacity(32 + 1 + 1 + 1 + KEY_ID_LEN + 1 + name.len());
    aad.extend_from_slice(b"taygedo/storage/aad/v1");
    aad.push(0x00); // 域分隔符，防止"标签后缀"与后续字段拼接产生歧义
    aad.push(h.version);
    aad.push(h.alg);
    aad.extend_from_slice(&h.key_id);
    aad.push(name.len() as u8);
    aad.extend_from_slice(name);
    aad
}

/// 把 16 字节 Key ID 渲染为十六进制 ASCII（32 字符）。
pub fn hex16(bytes: &[u8; KEY_ID_LEN]) -> String {
    let mut s = String::with_capacity(KEY_ID_LEN * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
