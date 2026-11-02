//! Unix / OpenWrt 上的主密钥文件读写（0600、root 所有、非链接）。
//!
//! 这是**明确的降级方案**：密钥以明文形式存在于磁盘，安全性完全依赖
//! 文件权限与运行账户隔离。它不等价于 TPM 封装，也不宣称能抵御
//! 取得 root 或物理接触磁盘的攻击者。

use std::path::Path;

use super::super::atomic;
use super::super::StorageError;
use super::KEY_BLOB_LEN;

/// 读取密钥文件；不存在返回 `Ok(None)`。
///
/// 强制要求：普通文件、非符号链接、硬链接数 1、权限 `0600`、属主为当前
/// 有效用户（以 root 运行为常态）。任何一条不满足都返回错误——**不做**
/// "权限宽就自动 chmod 后继续"的静默修复，避免掩盖部署错误。
pub fn read_key_file(path: &Path) -> Result<Option<Vec<u8>>, StorageError> {
    atomic::read_secure(path, KEY_BLOB_LEN, true)
}

/// 写入密钥文件（0600）。
///
/// 已存在则拒绝覆盖：主密钥覆盖等于把既有密文全部变成不可解。
pub fn write_key_file(path: &Path, blob: &[u8]) -> Result<(), StorageError> {
    if path.exists() {
        return Err(StorageError::MasterKeyCorrupt(format!(
            "{} 已存在，拒绝覆盖（覆盖会使既有密文永久不可解）",
            path.display()
        )));
    }
    atomic::write_atomic(path, blob, 0o600)
}
