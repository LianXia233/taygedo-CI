//! 平台抽象层：业务代码与操作系统实现之间的唯一边界。
//!
//! ## 约束
//!
//! - **只有本模块（含 `windows.rs` / `unix.rs` / `unsupported.rs`）允许出现
//!   `cfg(windows)` / `cfg(unix)` 等操作系统条件编译**；业务模块一律通过
//!   [`current()`] 取得平台实现，不得直接调用 OS API。
//! - 抽象按能力划分：[`KeyStore`]（主密钥保护与轮换）、
//!   [`fs::SecureFile`]（原子写入与 fd 级安全校验）、
//!   [`DesktopIntegration`]（打开 URL、子进程标志、宿主探测）。
//! - 平台选择在**编译期**完成，运行期无探测开销；`current()` 返回
//!   `&'static dyn Platform`，调用方不需要持有任何平台状态。
//! - macOS 复用 Unix 实现（低成本兼容），仅在打开 URL 时选择 `open`。
//! - 磁盘格式保持字节级兼容：`TGDKEY01` 密钥 blob、DPAPI blob、
//!   0600 文件布局与旧版本完全一致，重构不改变任何已有数据目录。
//!
//! ## 依赖方向
//!
//! ```text
//! service / store / web / crypto_storage
//!            │
//!            ▼
//!        platform（本层，唯一 OS 边界）
//!            │
//!     ┌──────┼───────┐
//!   windows unix  unsupported
//! ```

// 平台层内部分常量、编码函数与能力方法只在**其它**平台的实现里被引用
// （例如 Unix 密钥 blob 在 Windows 构建下不会出现）。这些是跨平台共存
// 契约的一部分，不是疏漏；统一放行，避免 `-D warnings` 下产生噪声。
#![allow(dead_code)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod fs;
#[cfg(all(unix, not(windows)))]
mod unix;
#[cfg(not(any(unix, windows)))]
mod unsupported;
#[cfg(windows)]
mod windows;

/// 宿主平台类型（运行期探测结果，仅用于日志与策略判定）。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HostKind {
    Windows,
    /// 具备 systemd 凭据设施的通用 Linux。
    Linux,
    /// OpenWrt / ImmortalWrt（procd，无 systemd）。
    OpenWrt,
    /// macOS：复用 Unix 文件与权限实现。
    MacOS,
    /// 其它 POSIX/非 POSIX 平台：能力显式降级，不静默假设。
    Other,
}

/// 主密钥保护方式。取值即事实，不做美化。
///
/// 五个变体跨平台共存：某个平台上"不被构造"是正常的，但枚举必须完整，
/// 否则日志文案与跨平台代码就得各自维护一份。
#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Protection {
    SystemdCredential,
    UnixFile,
    OpenWrtFile,
    DpapiUser,
    DpapiMachine,
}

impl Protection {
    pub fn as_str(self) -> &'static str {
        match self {
            Protection::SystemdCredential => "systemd-credential",
            Protection::UnixFile => "unix-file-0600",
            Protection::OpenWrtFile => "openwrt-file-0600",
            Protection::DpapiUser => "windows-dpapi-user",
            Protection::DpapiMachine => "windows-dpapi-machine",
        }
    }

    pub fn os_managed(self) -> bool {
        matches!(
            self,
            Protection::SystemdCredential | Protection::DpapiUser | Protection::DpapiMachine
        )
    }
}

// ---- 磁盘格式常量（跨平台共用，字节布局不可更改） -------------------------

pub const KEY_DIR_NAME: &str = "keys";
pub const KEY_FILE_NAME: &str = "master.key";
pub const DPAPI_FILE_NAME: &str = "master.key.dpapi";
pub const PENDING_KEY_FILE_NAME: &str = "master.key.pending";
pub const KEY_MAGIC: [u8; 8] = *b"TGDKEY01";
pub const KEY_FORMAT_VERSION: u8 = 1;
pub const KEY_BLOB_LEN: usize = 42;
pub const CREDENTIAL_NAME: &str = "taygedo_storage_key";

pub fn keys_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(KEY_DIR_NAME)
}

/// 编码 Unix 密钥文件（Magic 8 + 版本 1 + 保护方式 1 + 密钥 32）。
pub fn encode_key_blob(key: &[u8; 32], protection: Protection) -> Vec<u8> {
    let mut out = Vec::with_capacity(KEY_BLOB_LEN);
    out.extend_from_slice(&KEY_MAGIC);
    out.push(KEY_FORMAT_VERSION);
    out.push(match protection {
        Protection::UnixFile => 1,
        Protection::OpenWrtFile => 2,
        Protection::SystemdCredential => 3,
        Protection::DpapiUser => 4,
        Protection::DpapiMachine => 5,
    });
    out.extend_from_slice(key);
    out
}

/// 解码密钥文件。长度/Magic/版本不符一律视为损坏，绝不按明文猜测。
pub fn decode_key_blob(blob: &[u8]) -> Result<[u8; 32], PlatformError> {
    if blob.len() != KEY_BLOB_LEN {
        return Err(PlatformError::KeyCorrupt(format!(
            "密钥文件长度 {} 应为 {}",
            blob.len(),
            KEY_BLOB_LEN
        )));
    }
    if blob[..8] != KEY_MAGIC {
        return Err(PlatformError::KeyCorrupt(
            "密钥文件 Magic 不匹配（可能被替换或损坏）".into(),
        ));
    }
    if blob[8] != KEY_FORMAT_VERSION {
        return Err(PlatformError::KeyCorrupt(format!(
            "密钥文件版本 {} 不受支持",
            blob[8]
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&blob[10..42]);
    Ok(key)
}

// ---- 平台错误（与存储层 StorageError 解耦） --------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformError {
    Io(String),
    Corrupt(String),
    KeyCorrupt(String),
    TooLarge(usize),
    Unsupported(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::Io(m) => write!(f, "平台 I/O 错误：{m}"),
            PlatformError::Corrupt(m) => write!(f, "文件不安全：{m}"),
            PlatformError::KeyCorrupt(m) => write!(f, "主密钥不可用：{m}"),
            PlatformError::TooLarge(n) => write!(f, "文件过大（{n} 字节）"),
            PlatformError::Unsupported(m) => write!(f, "当前平台不支持：{m}"),
        }
    }
}

impl std::error::Error for PlatformError {}

impl From<std::io::Error> for PlatformError {
    fn from(e: std::io::Error) -> Self {
        PlatformError::Io(e.to_string())
    }
}

// ---- 能力接口 --------------------------------------------------------------

/// 主密钥保护设施。
pub trait KeyStore: Send + Sync {
    fn load_master_key(
        &self,
        data_dir: &Path,
    ) -> Result<Option<([u8; 32], Protection)>, PlatformError>;
    fn store_new_master_key(
        &self,
        data_dir: &Path,
        key: &[u8; 32],
    ) -> Result<Protection, PlatformError>;
    fn read_pending_key(&self, data_dir: &Path) -> Result<Option<[u8; 32]>, PlatformError>;
    fn write_pending_key(&self, data_dir: &Path, key: &[u8; 32]) -> Result<(), PlatformError>;
    fn promote_pending_key(&self, data_dir: &Path) -> Result<(), PlatformError>;
    /// 密钥环文件权限的诊断信息（无该能力时返回 None）。
    fn permission_warning(&self, keyring_path: &Path) -> Option<String>;
}

/// 桌面/宿主集成能力。
pub trait DesktopIntegration: Send + Sync {
    /// 用系统默认浏览器/处理器打开 URL。不依赖任何 Shell。
    fn open_url(&self, url: &str) -> Result<(), PlatformError>;
    /// 在派生子进程前应用平台标志（如 Windows 的 CREATE_NO_WINDOW）。
    fn configure_command(&self, cmd: &mut Command);
    fn host_kind(&self) -> HostKind;
}

/// 平台聚合根。
pub trait Platform: Send + Sync {
    fn os_name(&self) -> &'static str;
    fn host_kind(&self) -> HostKind;
    /// 是否存在 OS 级密钥设施（用于启动日志给出准确提示）。
    fn supports_os_keystore(&self) -> bool;
    /// 是否支持 `--rotate-storage-key` 的 pending 文件流程。
    fn supports_key_rotation(&self) -> bool;
    fn key_store(&self) -> &'static dyn KeyStore;
    fn secure_file(&self) -> &'static dyn fs::SecureFile;
    fn desktop(&self) -> &'static dyn DesktopIntegration;
}

/// 取得当前平台实现。编译期静态分发，无运行期探测与分配。
pub fn current() -> &'static dyn Platform {
    #[cfg(windows)]
    {
        &windows::PLATFORM
    }
    #[cfg(all(unix, not(windows)))]
    {
        &unix::PLATFORM
    }
    #[cfg(not(any(unix, windows)))]
    {
        &unsupported::PLATFORM
    }
}

// ---- 平台层内部共享的自由函数（供各实现复用） ------------------------------

pub(crate) fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), PlatformError> {
    current().secure_file().write_atomic(path, data, mode)
}

pub(crate) fn read_secure(
    path: &Path,
    max_len: usize,
    require_private: bool,
) -> Result<Option<Vec<u8>>, PlatformError> {
    current()
        .secure_file()
        .read_secure(path, max_len, require_private)
}

pub(crate) fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    current().secure_file().fsync_dir(dir)
}

pub fn cleanup_stale_temp(dir: &Path) -> usize {
    current().secure_file().cleanup_stale_temp(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_blob_roundtrip() {
        let key = [0x42u8; 32];
        let blob = encode_key_blob(&key, Protection::UnixFile);
        assert_eq!(blob.len(), KEY_BLOB_LEN);
        assert_eq!(decode_key_blob(&blob).unwrap(), key);
    }

    #[test]
    fn key_blob_rejects_short_and_bad_magic() {
        assert!(decode_key_blob(&[0u8; 10]).is_err());
        let mut blob = encode_key_blob(&[7u8; 32], Protection::UnixFile);
        blob[0] ^= 0xff;
        assert!(decode_key_blob(&blob).is_err());
    }

    #[test]
    fn protection_labels_are_stable() {
        assert_eq!(Protection::UnixFile.as_str(), "unix-file-0600");
        assert_eq!(Protection::OpenWrtFile.as_str(), "openwrt-file-0600");
        assert!(Protection::DpapiUser.os_managed());
        assert!(!Protection::UnixFile.os_managed());
    }
}
