//! 主密钥生命周期与文件专用密钥派生。
//!
//! ## 引导顺序（避免"鸡生蛋"）
//!
//! ```text
//! Store::new(data_dir)
//!   ├─ 1. 取数据目录排他锁（防并发迁移/写入）
//!   ├─ 2. 清理残留临时文件
//!   ├─ 3. 扫描数据目录：是否已存在密文？（只看 Magic，不解析内容）
//!   ├─ 4. KeyManager::load_or_init(has_existing_ciphertext)
//!   │       ├─ 有主密钥 → 复用（无论是否有密文）
//!   │       ├─ 无主密钥 + 无密文 → 生成并落盘（首次启动）
//!   │       └─ 无主密钥 + 有密文 → **报错**（绝不生成新密钥覆盖旧数据）
//!   └─ 5. 迁移旧版明文 → 之后才能读业务配置
//! ```
//!
//! 关键点：主密钥的获取**不依赖任何业务配置文件**。它来自 systemd 凭据 /
//! DPAPI / 独立密钥文件，三者都在读 `config.json` 之前就绪，因此不存在
//! "要读 config 才拿到密钥、要密钥才读 config"的循环依赖。
//!
//! ## 密钥丢失 / 损坏的处理
//!
//! | 情况 | 行为 |
//! |---|---|
//! | 密钥文件不存在 + 无密文 | 生成新密钥（首次启动） |
//! | 密钥文件不存在 + 有密文 | 报 `MasterKeyMissing`，拒绝启动 |
//! | 密钥文件存在但 Magic/长度/权限不对 | 报 `MasterKeyCorrupt`，拒绝启动 |
//! | 密钥正常但密文 Key ID 不符 | 报 `KeyIdMismatch`，拒绝启动 |
//!
//! 三种错误都不会自动"修复"——因为自动修复的实质是丢数据。

use std::path::Path;

use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::platform::{self, Protection};
use super::{envelope, FileIdentity, StorageError};

/// 文件密钥派生的域分隔前缀。
const INFO_FILE: &[u8] = b"taygedo/storage/file/v1/";
/// Key ID 计算域分隔。
const KEY_ID_DOMAIN: &[u8] = b"taygedo/storage/key-id/v1";

/// 主密钥管理器。
pub struct KeyManager {
    /// 32 字节主密钥。`Zeroizing` 保证 drop 时清零。
    master: Zeroizing<[u8; 32]>,
    /// 16 字节 Key ID（ASCII 十六进制），写进每个密文头部。
    key_id: [u8; envelope::KEY_ID_LEN],
    protection: Protection,
}

impl KeyManager {
    /// 加载已有主密钥，或在确认"没有任何密文"时生成新密钥。
    ///
    /// `has_existing_ciphertext` 由调用方扫描数据目录得出（只看 Magic）。
    pub fn load_or_init(
        data_dir: &Path,
        has_existing_ciphertext: bool,
    ) -> Result<Self, StorageError> {
        match platform::load_master_key(data_dir)? {
            Some((master, protection)) => {
                if master.iter().all(|b| *b == 0) {
                    return Err(StorageError::MasterKeyCorrupt(
                        "主密钥为全零（非法）".into(),
                    ));
                }
                Ok(Self::from_master(master, protection))
            }
            None => {
                if has_existing_ciphertext {
                    return Err(StorageError::MasterKeyMissing);
                }
                let mut master = Zeroizing::new([0u8; 32]);
                rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut master[..]);
                let protection = platform::store_new_master_key(data_dir, &master)?;
                Ok(Self::from_master(master, protection))
            }
        }
    }

    /// 由已知主密钥构造（测试与密钥轮换使用）。
    pub fn from_master(master: Zeroizing<[u8; 32]>, protection: Protection) -> Self {
        let key_id = compute_key_id(&master);
        Self {
            master,
            key_id,
            protection,
        }
    }

    pub fn key_id(&self) -> &[u8; envelope::KEY_ID_LEN] {
        &self.key_id
    }

    /// 导出主密钥副本（仅用于密钥轮换落盘，调用方必须立即用毕即弃）。
    pub fn export_master(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(*self.master)
    }

    pub fn key_id_hex(&self) -> String {
        envelope::hex16(&self.key_id)
    }

    pub fn protection(&self) -> Protection {
        self.protection
    }

    /// 派生文件专用密钥：`HKDF-SHA256(ikm=master, salt=file_salt, info=域||身份)`。
    ///
    /// 每个文件、每次写入都有独立 salt，因此同一主密钥下不同文件/不同版本
    /// 的文件密钥互不相同（密钥隔离）。
    pub fn derive_file_key(
        &self,
        id: FileIdentity,
        salt: &[u8; envelope::SALT_LEN],
    ) -> Zeroizing<[u8; 32]> {
        let mut info = Vec::with_capacity(INFO_FILE.len() + 8);
        info.extend_from_slice(INFO_FILE);
        info.extend_from_slice(id.as_str().as_bytes());

        let hk = Hkdf::<Sha256>::new(Some(salt), &self.master[..]);
        let mut okm = [0u8; 32];
        hk.expand(&info, &mut okm)
            .expect("HKDF 输出 32 字节必然合法");
        Zeroizing::new(okm)
    }
}

/// Key ID = `hex(SHA-256("taygedo/storage/key-id/v1" || master)[..8])`。
///
/// 单向、非机密：泄露 Key ID 不能反推主密钥，但足以让运维区分
/// "这份密文是哪一代密钥加的"。
pub fn compute_key_id(master: &[u8; 32]) -> [u8; envelope::KEY_ID_LEN] {
    let mut h = Sha256::new();
    h.update(KEY_ID_DOMAIN);
    h.update(master);
    let digest = h.finalize();
    let mut out = [0u8; envelope::KEY_ID_LEN];
    for i in 0..8 {
        let hex = format!("{:02x}", digest[i]);
        out[i * 2] = hex.as_bytes()[0];
        out[i * 2 + 1] = hex.as_bytes()[1];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn km(seed: u8) -> KeyManager {
        KeyManager::from_master(Zeroizing::new([seed; 32]), Protection::UnixFile)
    }

    #[test]
    fn key_id_is_stable_and_domain_separated() {
        assert_eq!(km(1).key_id_hex(), km(1).key_id_hex());
        assert_ne!(km(1).key_id_hex(), km(2).key_id_hex());
        assert_eq!(km(1).key_id_hex().len(), 32); // 16 字节 ASCII 十六进制
        assert!(km(1).key_id_hex().bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn file_keys_differ_by_identity_and_salt() {
        let k = km(7);
        let salt = [9u8; 32];
        let a = k.derive_file_key(FileIdentity::Accounts, &salt);
        let b = k.derive_file_key(FileIdentity::Config, &salt);
        let c = k.derive_file_key(FileIdentity::Accounts, &[8u8; 32]);
        assert_ne!(a[..], b[..]);
        assert_ne!(a[..], c[..]);
        // 同输入必须确定
        assert_eq!(a[..], k.derive_file_key(FileIdentity::Accounts, &salt)[..]);
    }

    #[test]
    fn different_master_gives_different_file_key() {
        let salt = [1u8; 32];
        let a = km(1).derive_file_key(FileIdentity::Config, &salt);
        let b = km(2).derive_file_key(FileIdentity::Config, &salt);
        assert_ne!(a[..], b[..]);
    }
}
