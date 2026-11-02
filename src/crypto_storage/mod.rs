//! 配置文件静态加密存储层。
//!
//! ## 目标
//!
//! 让磁盘上的所有敏感持久化配置文件（`accounts.json` / `config.json` /
//! `state.json` / `keyring.json`）以**密文**形式落盘，同时**不改变** `Store`
//! 对上层暴露的读写语义与业务数据结构。
//!
//! ## 分层
//!
//! ```text
//! Store（业务语义：load_accounts/save_config/...）
//!   └─ StorageCrypto（seal / open：把 bytes 变成密文，反之亦然）
//!        ├─ envelope   统一密文格式（Magic/Version/KeyID/Salt/Nonce/密文+Tag）
//!        ├─ key_manager 主密钥 + HKDF-SHA256 文件专用密钥派生 + Key ID
//!        ├─ platform   三平台主密钥保护（systemd 凭据 / DPAPI / 0600 文件）
//!        ├─ atomic     安全原子写入、基于 fd 的安全读取
//!        └─ migration  旧版明文自动迁移 + 加密备份 + 迁移日志
//! ```
//!
//! ## 密码学
//!
//! - AEAD：**XChaCha20-Poly1305**（RustCrypto `chacha20poly1305`，24 字节随机 nonce）。
//! - KDF：**HKDF-SHA256**。主密钥为 IKM，每文件随机 32 字节 Salt 为 salt，
//!   `info = "taygedo/storage/file/v1/" || <文件逻辑身份>`，输出 32 字节文件密钥。
//! - 随机数：`rand::rngs::OsRng`（Linux `getrandom(2)` / Windows `BCryptGenRandom`）。
//! - AAD 绑定：格式版本 + 算法 + Key ID + 文件逻辑身份。因此把 `accounts` 的
//!   密文拷到 `config.json` 上会认证失败，不同文件之间无法互相顶替。
//! - Key ID：`SHA-256("taygedo/storage/key-id/v1" || master)[..8]` 的十六进制，
//!   非机密、可公开，用于区分主密钥世代（轮换时新旧密文可并存）。
//!
//! ## 明确不做的事
//!
//! - 不承诺 `unlink` 能物理擦除历史明文（SSD / flash / CoW 文件系统上不成立）。
//! - 不把普通文件权限（0600）描述成"绝对防破解"：它只在"攻击者未取得
//!   root/同账户权限"的前提下有效。

pub mod atomic;
pub mod envelope;
pub mod key_manager;
pub mod migration;
pub mod platform;

use std::fmt;

use rand::RngCore;
use zeroize::Zeroizing;

pub use envelope::{HEADER_LEN, MAX_FILE_BYTES, NONCE_LEN, SALT_LEN, TAG_LEN};
pub use key_manager::KeyManager;
pub use platform::Protection;

/// 当前统一的密文格式版本。未知版本一律拒绝处理（不尝试猜测）。
pub const FORMAT_VERSION: u8 = envelope::FORMAT_VERSION;

/// 明文上限（比整文件上限小一个头部 + Tag，留出余量）。
pub const MAX_PLAINTEXT_BYTES: usize = MAX_FILE_BYTES - HEADER_LEN - TAG_LEN;

/// 受管文件的**逻辑身份**。
///
/// 该值进入 AAD 与 HKDF info，是"这段密文属于哪个文件"的密码学绑定，
/// 而不是靠文件名判断——因此重命名或移动文件不会让密文变得可信。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileIdentity {
    Accounts,
    Config,
    State,
    KeyRing,
}

impl FileIdentity {
    /// 全部受管文件。迁移、密钥轮换、诊断都以本表为准。
    pub const ALL: [FileIdentity; 4] = [
        FileIdentity::Accounts,
        FileIdentity::Config,
        FileIdentity::State,
        FileIdentity::KeyRing,
    ];

    /// AAD / HKDF 中使用的稳定逻辑名（**不可随意修改**，改了旧密文就解不开）。
    pub fn as_str(self) -> &'static str {
        match self {
            FileIdentity::Accounts => "accounts",
            FileIdentity::Config => "config",
            FileIdentity::State => "state",
            FileIdentity::KeyRing => "keyring",
        }
    }

    /// 磁盘文件名。
    pub fn file_name(self) -> &'static str {
        match self {
            FileIdentity::Accounts => "accounts.json",
            FileIdentity::Config => "config.json",
            FileIdentity::State => "state.json",
            FileIdentity::KeyRing => "keyring.json",
        }
    }
}

/// 存储层错误。
///
/// 所有变体都携带足够定位问题的信息，但**不包含任何密钥材料或明文片段**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// I/O 失败（创建目录、读写、rename 等）。
    Io(String),
    /// 密文头部 Magic 不匹配——不是本方案的密文文件。
    BadMagic,
    /// 未知格式版本：拒绝处理，绝不"尽力解析"。
    UnsupportedVersion(u8),
    /// 未知加密算法标识。
    UnsupportedAlg(u8),
    /// 文件被截断、长度与头部声明不符、或存在尾随垃圾。
    Corrupt(String),
    /// 文件超过长度上限，拒绝读入（防内存耗尽）。
    TooLarge(usize),
    /// AEAD 认证失败：密文被篡改，或主密钥不对。
    AuthFailed,
    /// 密文 Key ID 与当前主密钥不匹配（密钥丢失/被替换/未轮换到位）。
    KeyIdMismatch { expected: String, found: String },
    /// 主密钥缺失，但已存在加密数据（**不得**自动生成新密钥覆盖）。
    MasterKeyMissing,
    /// 主密钥文件损坏 / 长度非法 / 权限不安全。
    MasterKeyCorrupt(String),
    /// 明文数据未通过结构校验（迁移前必须验证）。
    InvalidPlaintext(String),
    /// 平台主密钥保护设施不可用。
    Platform(String),
    /// 并发锁被占用（另一个实例正在运行）。
    Locked(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "存储 I/O 错误：{e}"),
            StorageError::BadMagic => write!(f, "文件不是 taygedo 加密配置（Magic 不匹配）"),
            StorageError::UnsupportedVersion(v) => {
                write!(f, "不支持的存储格式版本 {v}（支持版本 {}）", FORMAT_VERSION)
            }
            StorageError::UnsupportedAlg(a) => write!(f, "不支持的加密算法标识 {a}"),
            StorageError::Corrupt(m) => write!(f, "密文文件损坏：{m}"),
            StorageError::TooLarge(n) => write!(f, "文件过大（{n} 字节），已拒绝读入"),
            StorageError::AuthFailed => {
                write!(f, "密文认证失败：文件被篡改，或当前主密钥与该文件不匹配")
            }
            StorageError::KeyIdMismatch { expected, found } => write!(
                f,
                "密钥 ID 不匹配：文件由 {found} 加密，当前主密钥为 {expected}；\
                 请恢复正确的主密钥（切勿删除密文）"
            ),
            StorageError::MasterKeyMissing => write!(
                f,
                "主密钥缺失，但数据目录中已存在加密配置。为避免不可逆的数据丢失，\
                 服务拒绝自动生成新密钥。请恢复主密钥备份后再启动"
            ),
            StorageError::MasterKeyCorrupt(m) => write!(f, "主密钥不可用：{m}"),
            StorageError::InvalidPlaintext(m) => write!(f, "明文配置结构非法：{m}"),
            StorageError::Platform(m) => write!(f, "平台密钥保护不可用：{m}"),
            StorageError::Locked(m) => write!(f, "数据目录被占用：{m}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        StorageError::Io(e.to_string())
    }
}

/// 存储加密门面：持有主密钥，负责单文件的 seal / open。
pub struct StorageCrypto {
    keys: KeyManager,
}

impl StorageCrypto {
    pub fn new(keys: KeyManager) -> Self {
        Self { keys }
    }

    pub fn key_id_hex(&self) -> String {
        self.keys.key_id_hex()
    }

    pub fn protection(&self) -> Protection {
        self.keys.protection()
    }

    /// 加密一段明文，返回完整密文文件字节（含头部）。
    ///
    /// 每次调用都生成**全新的** Salt 与 Nonce，因此同一明文两次加密的
    /// 结果必然不同（语义安全性）。
    pub fn seal(&self, id: FileIdentity, plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return Err(StorageError::TooLarge(plaintext.len()));
        }
        let mut salt = [0u8; SALT_LEN];
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut salt);
        rand::rngs::OsRng.fill_bytes(&mut nonce);

        let file_key = self.keys.derive_file_key(id, &salt);

        let mut header = envelope::Header {
            version: FORMAT_VERSION,
            alg: envelope::ALG_XCHACHA20_POLY1305_HKDF_SHA256,
            key_id: *self.keys.key_id(),
            salt,
            nonce,
            ct_len: 0,
        };
        let aad = envelope::build_aad(id, &header);

        use chacha20poly1305::aead::{Aead, Payload};
        use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
        let cipher = XChaCha20Poly1305::new_from_slice(&file_key[..])
            .map_err(|_| StorageError::Corrupt("文件密钥长度非法".into()))?;
        let ct = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| StorageError::AuthFailed)?;

        header.ct_len = u32::try_from(ct.len()).map_err(|_| StorageError::TooLarge(ct.len()))?;
        let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
        out.extend_from_slice(&header.serialize());
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// 解密一段密文文件字节，返回明文（缓冲区在 drop 时清零）。
    pub fn open(&self, id: FileIdentity, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, StorageError> {
        let env = envelope::parse(blob)?;
        if env.header.key_id != *self.keys.key_id() {
            return Err(StorageError::KeyIdMismatch {
                expected: self.keys.key_id_hex(),
                found: envelope::hex16(&env.header.key_id),
            });
        }
        let file_key = self.keys.derive_file_key(id, &env.header.salt);
        let aad = envelope::build_aad(id, &env.header);

        use chacha20poly1305::aead::{Aead, Payload};
        use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
        let cipher = XChaCha20Poly1305::new_from_slice(&file_key[..])
            .map_err(|_| StorageError::Corrupt("文件密钥长度非法".into()))?;
        let pt = cipher
            .decrypt(
                XNonce::from_slice(&env.header.nonce),
                Payload {
                    msg: env.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| StorageError::AuthFailed)?;
        Ok(Zeroizing::new(pt))
    }
}

#[cfg(test)]
mod tests;
