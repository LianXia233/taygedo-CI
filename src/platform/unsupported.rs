//! 其它平台：显式声明"不具备密钥托管能力"，不做静默降级。
//!
//! 保留可编译的最小实现，便于未来新增平台时先跑通构建再逐项补齐能力；
//! 任何涉及主密钥的操作都会返回明确的 `Unsupported` 错误。

use std::fs::{File, Metadata, OpenOptions};
use std::path::Path;
use std::process::Command;

use super::fs::SecureFile;
use super::{DesktopIntegration, HostKind, KeyStore, Platform, PlatformError, Protection};

pub(super) static PLATFORM: UnsupportedPlatform = UnsupportedPlatform;
static KEY_STORE: UnsupportedKeyStore = UnsupportedKeyStore;
static SECURE_FILE: UnsupportedSecureFile = UnsupportedSecureFile;
static DESKTOP: UnsupportedDesktop = UnsupportedDesktop;

pub(super) struct UnsupportedPlatform;

fn unsupported(what: &str) -> PlatformError {
    PlatformError::Unsupported(format!("当前平台尚未实现 {what}"))
}

impl Platform for UnsupportedPlatform {
    fn os_name(&self) -> &'static str {
        "unsupported"
    }
    fn host_kind(&self) -> HostKind {
        HostKind::Other
    }
    fn supports_os_keystore(&self) -> bool {
        false
    }
    fn supports_key_rotation(&self) -> bool {
        false
    }
    fn key_store(&self) -> &'static dyn KeyStore {
        &KEY_STORE
    }
    fn secure_file(&self) -> &'static dyn SecureFile {
        &SECURE_FILE
    }
    fn desktop(&self) -> &'static dyn DesktopIntegration {
        &DESKTOP
    }
}

struct UnsupportedKeyStore;

impl KeyStore for UnsupportedKeyStore {
    fn load_master_key(
        &self,
        _data_dir: &Path,
    ) -> Result<Option<([u8; 32], Protection)>, PlatformError> {
        Err(unsupported("主密钥保护机制"))
    }
    fn store_new_master_key(
        &self,
        _data_dir: &Path,
        _key: &[u8; 32],
    ) -> Result<Protection, PlatformError> {
        Err(unsupported("主密钥保护机制"))
    }
    fn read_pending_key(&self, _data_dir: &Path) -> Result<Option<[u8; 32]>, PlatformError> {
        Ok(None)
    }
    fn write_pending_key(&self, _data_dir: &Path, _key: &[u8; 32]) -> Result<(), PlatformError> {
        Err(unsupported("主密钥轮换"))
    }
    fn promote_pending_key(&self, _data_dir: &Path) -> Result<(), PlatformError> {
        Ok(())
    }
    fn permission_warning(&self, _keyring_path: &Path) -> Option<String> {
        None
    }
}

struct UnsupportedSecureFile;

impl SecureFile for UnsupportedSecureFile {
    fn open_new(
        &self,
        opts: &mut OpenOptions,
        path: &Path,
        _mode: u32,
    ) -> Result<File, std::io::Error> {
        opts.open(path)
    }
    fn open_existing(&self, opts: &mut OpenOptions, path: &Path) -> Result<File, std::io::Error> {
        opts.open(path)
    }
    fn validate_metadata(
        &self,
        _path: &Path,
        _meta: &Metadata,
        _require_private: bool,
    ) -> Result<(), PlatformError> {
        Ok(())
    }
    fn fsync_dir(&self, _dir: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

struct UnsupportedDesktop;

impl DesktopIntegration for UnsupportedDesktop {
    fn open_url(&self, _url: &str) -> Result<(), PlatformError> {
        Err(unsupported("打开系统浏览器"))
    }
    fn configure_command(&self, _cmd: &mut Command) {}
    fn host_kind(&self) -> HostKind {
        HostKind::Other
    }
}
