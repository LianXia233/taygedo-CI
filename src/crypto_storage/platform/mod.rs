//! 平台主密钥保护。
//!
//! 三个平台使用**同一份**密文格式（见 [`super::envelope`]），但主密钥的
//! 保护方式各自独立，互不通用：
//!
//! | 平台 | 主密钥来源 | 说明 |
//! |---|---|---|
//! | Debian 13 | systemd `LoadCredentialEncrypted=` 注入的凭据 | 若 unit 用 TPM2 封装，密钥由 TPM 解封；本程序**不**声称自己实现了 TPM 保护 |
//! | Debian 13（无凭据设施） | `keys/master.key`，0600、root 所有 | 明确的降级方案，见 README |
//! | Windows x64 | DPAPI（当前服务账户上下文） | 密文绑定账户，拷到别的账户/机器上解不开 |
//! | OpenWrt | `keys/master.key`，0600、root 所有 | 不依赖 systemd / TPM / 桌面密钥服务 |
//!
//! **绝不**把普通明文密钥文件包装成"TPM 保护密钥"：`Protection` 的取值
//! 如实反映实际保护等级，并在启动日志中原样输出。

use std::path::{Path, PathBuf};

use zeroize::Zeroizing;

// 这些模块只在 Unix 分支被引用（Windows 走 DPAPI）。不按平台裁剪会让
// Windows 构建出现大量 "never used" 告警，掩盖真正的死代码。
#[cfg(unix)]
use super::atomic;
use super::StorageError;

#[cfg(unix)]
pub mod linux;
#[cfg(unix)]
pub mod openwrt;
#[cfg(unix)]
mod unix_file;
#[cfg(windows)]
pub mod windows;

/// 主密钥目录（相对数据目录）。
pub const KEY_DIR_NAME: &str = "keys";
/// 主密钥文件名（Unix / OpenWrt 的 0600 明文密钥文件）。
#[cfg(unix)]
pub const KEY_FILE_NAME: &str = "master.key";
/// Windows DPAPI 封装后的主密钥文件名。
#[cfg_attr(not(windows), allow(dead_code))]
pub const DPAPI_FILE_NAME: &str = "master.key.dpapi";
/// 密钥轮换时的待提升文件。存在即表示"轮换进行中"，启动时会自动续做。
#[cfg(unix)]
pub const PENDING_KEY_FILE_NAME: &str = "master.key.pending";
/// 主密钥文件 Magic。
#[cfg(unix)]
pub const KEY_MAGIC: [u8; 8] = *b"TGDKEY01";
/// 主密钥文件版本。
#[cfg(unix)]
pub const KEY_FORMAT_VERSION: u8 = 1;
/// 密钥文件长度（Magic 8 + 版本 1 + 保护方式 1 + 密钥 32）。
///
/// 非 Unix 平台只在测试里引用（Windows 的密钥走 DPAPI 封装），
/// 故对这些目标放行 dead_code。
#[cfg_attr(not(unix), allow(dead_code))]
pub const KEY_BLOB_LEN: usize = 42;

/// 主密钥保护方式。取值即事实，不做美化。
///
/// 四个变体跨平台共存：某个平台上"不被构造"是正常的（Linux 不会构造 DPAPI
/// 变体，Windows 不会构造凭据/文件变体），但枚举必须完整，否则跨平台代码
/// 与日志文案就得各自维护一份。因此这里统一放行 dead_code。
#[allow(dead_code)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Protection {
    /// 由 systemd 凭据注入（unit 可用 `LoadCredentialEncrypted=` 让 TPM2 解封）。
    SystemdCredential,
    /// Debian/通用 Linux：root 所有、0600 的独立密钥文件。
    UnixFile,
    /// OpenWrt：root 所有、0600 的独立密钥文件（无 systemd / TPM 依赖）。
    OpenWrtFile,
    /// Windows DPAPI，当前用户（服务账户）作用域。
    DpapiUser,
    /// Windows DPAPI，本机作用域（显式开启，安全边界更宽）。
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

    /// 是否由操作系统密钥设施（而非裸文件权限）保护。
    ///
    /// 注意：即便是 `true`，也只表示"密钥交给了 OS 设施保管"，
    /// 具体是否落到 TPM 取决于 systemd unit / 机器是否有 TPM。
    pub fn os_managed(self) -> bool {
        matches!(
            self,
            Protection::SystemdCredential | Protection::DpapiUser | Protection::DpapiMachine
        )
    }
}

/// 主密钥目录路径。
pub fn keys_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(KEY_DIR_NAME)
}

/// 编码密钥文件（Unix 路径使用）。
#[cfg(unix)]
fn encode_key_blob(key: &[u8; 32], protection: Protection) -> Vec<u8> {
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

/// 解码密钥文件。任何长度/Magic 不符都视为损坏——**不**尝试当明文读。
#[cfg(unix)]
fn decode_key_blob(blob: &[u8]) -> Result<[u8; 32], StorageError> {
    if blob.len() != KEY_BLOB_LEN {
        return Err(StorageError::MasterKeyCorrupt(format!(
            "密钥文件长度 {} 应为 {}",
            blob.len(),
            KEY_BLOB_LEN
        )));
    }
    if blob[..8] != KEY_MAGIC {
        return Err(StorageError::MasterKeyCorrupt(
            "密钥文件 Magic 不匹配（可能被替换或损坏）".into(),
        ));
    }
    if blob[8] != KEY_FORMAT_VERSION {
        return Err(StorageError::MasterKeyCorrupt(format!(
            "密钥文件版本 {} 不受支持",
            blob[8]
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&blob[10..42]);
    Ok(key)
}

/// 主密钥材料与其保护方式的组合。
///
/// 独立命名而非内联元组，一是避免类型签名过长，二是让"密钥**必须**与
/// 保护方式一起返回"这件事在类型层面显式化——不允许只拿到裸密钥而不知道
/// 它从哪来（那会导致日志/诊断里说不清密钥的保护强度）。
pub type MasterKeyMaterial = (Zeroizing<[u8; 32]>, Protection);

/// 读取主密钥。
///
/// - `Ok(None)`：**完全没有**密钥材料（首次启动）。调用方据此决定是否生成新密钥；
///   若数据目录中已存在密文，调用方必须报错而不是生成新密钥。
/// - `Ok(Some(..))`：拿到主密钥及保护方式。
/// - `Err(..)`：密钥存在但损坏 / 权限不安全 / 平台设施不可用。**不得**降级为"生成新密钥"。
pub fn load_master_key(data_dir: &Path) -> Result<Option<MasterKeyMaterial>, StorageError> {
    #[cfg(unix)]
    {
        // 1) systemd 凭据优先（Debian 13 推荐部署方式）。
        if let Some(res) = linux::credential_key()? {
            return Ok(Some((Zeroizing::new(res), Protection::SystemdCredential)));
        }
        // 2) 独立密钥文件（Debian 无凭据设施 / OpenWrt）。
        let path = keys_dir(data_dir).join(KEY_FILE_NAME);
        let blob = unix_file::read_key_file(&path)?;
        let Some(blob) = blob else { return Ok(None) };
        let key = decode_key_blob(&blob)?;
        let protection = if openwrt::is_openwrt() {
            Protection::OpenWrtFile
        } else {
            Protection::UnixFile
        };
        Ok(Some((Zeroizing::new(key), protection)))
    }
    #[cfg(windows)]
    {
        let path = keys_dir(data_dir).join(DPAPI_FILE_NAME);
        match windows::unprotect_file(&path)? {
            None => Ok(None),
            Some((key, protection)) => Ok(Some((Zeroizing::new(key), protection))),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = data_dir;
        Err(StorageError::Platform(
            "当前平台没有实现主密钥保护机制".into(),
        ))
    }
}

/// 生成并落盘一份新的主密钥。**仅在确认没有任何既有密文时调用**。
pub fn store_new_master_key(data_dir: &Path, key: &[u8; 32]) -> Result<Protection, StorageError> {
    let dir = keys_dir(data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| StorageError::Io(format!("创建密钥目录 {} 失败：{e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 目录本身也收紧到 0700：密钥文件名是固定的，目录可枚举即多一层风险。
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let protection = if openwrt::is_openwrt() {
            Protection::OpenWrtFile
        } else {
            Protection::UnixFile
        };
        let path = dir.join(KEY_FILE_NAME);
        unix_file::write_key_file(&path, &encode_key_blob(key, protection))?;
        Ok(protection)
    }
    #[cfg(windows)]
    {
        let _ = &dir;
        windows::protect_to_file(&dir.join(DPAPI_FILE_NAME), key)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (dir, key);
        Err(StorageError::Platform(
            "当前平台没有实现主密钥保护机制".into(),
        ))
    }
}

/// 当前平台是否可能提供 OS 级密钥设施（用于启动日志给出准确提示）。
pub fn platform_supports_os_keystore() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(unix)]
    {
        linux::credentials_dir().is_some()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

// ---------------------------------------------------------------------------
// 密钥轮换支持：pending 文件
// ---------------------------------------------------------------------------

/// 读取待提升的新主密钥。存在即表示上一次轮换未完成。
#[cfg(unix)]
pub fn read_pending_key(data_dir: &Path) -> Result<Option<Zeroizing<[u8; 32]>>, StorageError> {
    let path = keys_dir(data_dir).join(PENDING_KEY_FILE_NAME);
    let Some(blob) = unix_file::read_key_file(&path)? else {
        return Ok(None);
    };
    Ok(Some(Zeroizing::new(decode_key_blob(&blob)?)))
}

#[cfg(windows)]
pub fn read_pending_key(_data_dir: &Path) -> Result<Option<Zeroizing<[u8; 32]>>, StorageError> {
    Ok(None)
}

#[cfg(not(any(unix, windows)))]
pub fn read_pending_key(_data_dir: &Path) -> Result<Option<Zeroizing<[u8; 32]>>, StorageError> {
    Ok(None)
}

/// 把新主密钥写入 pending 文件（轮换第一步，确保新密钥不会丢）。
#[cfg(unix)]
pub fn write_pending_key(data_dir: &Path, master: &[u8; 32]) -> Result<(), StorageError> {
    let dir = keys_dir(data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| StorageError::Io(format!("创建密钥目录 {} 失败：{e}", dir.display())))?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let path = dir.join(PENDING_KEY_FILE_NAME);
    let protection = if openwrt::is_openwrt() {
        Protection::OpenWrtFile
    } else {
        Protection::UnixFile
    };
    let blob = encode_key_blob(master, protection);
    // 允许覆盖：pending 只是过渡产物，覆盖它不会让既有密文不可解。
    atomic::write_atomic(&path, &blob, 0o600)
}

#[cfg(not(unix))]
pub fn write_pending_key(_data_dir: &Path, _master: &[u8; 32]) -> Result<(), StorageError> {
    Err(StorageError::Platform(
        "当前平台未实现主密钥轮换（Windows 上请用 DPAPI 重新封装后手工替换）".into(),
    ))
}

/// 把 pending 提升为正式主密钥（原子 rename 覆盖）。
#[cfg(unix)]
pub fn promote_pending_key(data_dir: &Path) -> Result<(), StorageError> {
    let dir = keys_dir(data_dir);
    let from = dir.join(PENDING_KEY_FILE_NAME);
    let to = dir.join(KEY_FILE_NAME);
    std::fs::rename(&from, &to)
        .map_err(|e| StorageError::Io(format!("提升待用主密钥失败：{e}")))?;
    atomic::fsync_dir(&dir).ok();
    Ok(())
}

#[cfg(not(unix))]
pub fn promote_pending_key(_data_dir: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 这两个用例测的是 Unix 密钥文件编码（`encode_key_blob` / `decode_key_blob`
    // 本身只在 unix 下编译），Windows 上引用会直接编译失败。
    #[cfg(unix)]
    #[test]
    fn key_blob_roundtrip() {
        let key = [0x42u8; 32];
        let blob = encode_key_blob(&key, Protection::UnixFile);
        assert_eq!(blob.len(), KEY_BLOB_LEN);
        assert_eq!(decode_key_blob(&blob).unwrap(), key);
    }

    #[cfg(unix)]
    #[test]
    fn key_blob_rejects_short_and_bad_magic() {
        assert!(decode_key_blob(&[0u8; 10]).is_err());
        let mut blob = encode_key_blob(&[7u8; 32], Protection::UnixFile);
        blob[0] ^= 0xff;
        assert!(decode_key_blob(&blob).is_err());
        let mut blob2 = encode_key_blob(&[7u8; 32], Protection::UnixFile);
        blob2[8] = 9;
        assert!(decode_key_blob(&blob2).is_err());
    }

    #[test]
    fn protection_labels_are_stable() {
        // 这些字符串会出现在启动日志与文档里，改名等同于破坏运维约定。
        assert_eq!(Protection::UnixFile.as_str(), "unix-file-0600");
        assert_eq!(Protection::OpenWrtFile.as_str(), "openwrt-file-0600");
        assert!(Protection::DpapiUser.os_managed());
        assert!(!Protection::UnixFile.os_managed());
    }
}
