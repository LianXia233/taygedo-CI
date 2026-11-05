//! 存储层 ↔ 平台抽象层的薄适配。
//!
//! 平台差异（DPAPI / systemd 凭据 / 0600 文件 / pending 轮换）已全部下沉到
//! [`crate::platform`]；本模块只负责把平台层返回的裸密钥包装为
//! `Zeroizing<[u8; 32]>`，并做 `PlatformError` → `StorageError` 的错误映射。
//! 磁盘格式保持字节级兼容，调用方 API 不变。

use std::path::Path;

use zeroize::Zeroizing;

use super::StorageError;
use crate::platform as sys;

// 磁盘格式常量在各形态下被使用的子集不同；保持统一导出以免调用点分叉。
#[allow(unused_imports)]
pub use crate::platform::{
    keys_dir, Protection, DPAPI_FILE_NAME, KEY_BLOB_LEN, KEY_DIR_NAME, KEY_FILE_NAME,
    KEY_FORMAT_VERSION, KEY_MAGIC, PENDING_KEY_FILE_NAME,
};

/// 主密钥材料与其保护方式的组合。
pub type MasterKeyMaterial = (Zeroizing<[u8; 32]>, Protection);

/// 读取主密钥。`Ok(None)` 表示首次启动（完全没有密钥材料）。
pub fn load_master_key(data_dir: &Path) -> Result<Option<MasterKeyMaterial>, StorageError> {
    let found = sys::current().key_store().load_master_key(data_dir)?;
    Ok(found.map(|(key, protection)| (Zeroizing::new(key), protection)))
}

/// 生成并落盘新主密钥（仅在确认没有任何既有密文时调用）。
pub fn store_new_master_key(data_dir: &Path, key: &[u8; 32]) -> Result<Protection, StorageError> {
    Ok(sys::current()
        .key_store()
        .store_new_master_key(data_dir, key)?)
}

/// 当前平台是否提供 OS 级密钥设施（启动日志据此给出准确提示）。
pub fn platform_supports_os_keystore() -> bool {
    sys::current().supports_os_keystore()
}

/// 读取待提升的新主密钥。存在即表示上一次轮换未完成。
pub fn read_pending_key(data_dir: &Path) -> Result<Option<Zeroizing<[u8; 32]>>, StorageError> {
    let found = sys::current().key_store().read_pending_key(data_dir)?;
    Ok(found.map(Zeroizing::new))
}

pub fn write_pending_key(data_dir: &Path, master: &[u8; 32]) -> Result<(), StorageError> {
    Ok(sys::current()
        .key_store()
        .write_pending_key(data_dir, master)?)
}

pub fn promote_pending_key(data_dir: &Path) -> Result<(), StorageError> {
    Ok(sys::current().key_store().promote_pending_key(data_dir)?)
}
