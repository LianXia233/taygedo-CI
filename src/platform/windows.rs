//! Windows 平台实现：DPAPI 主密钥保护、重解析点校验、桌面集成。
//!
//! 本文件是全仓库唯一允许直接调用 Win32 API 的位置。业务层只通过
//! `platform::current()` 的 trait 访问这些能力。

use std::fs::{File, Metadata, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};

use super::fs::SecureFile;
use super::{
    DesktopIntegration, HostKind, KeyStore, Platform, PlatformError, Protection, DPAPI_FILE_NAME,
};

const MAX_DPAPI_BLOB: usize = 4096;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

pub(super) static PLATFORM: WindowsPlatform = WindowsPlatform;
static KEY_STORE: WindowsKeyStore = WindowsKeyStore;
static SECURE_FILE: WindowsSecureFile = WindowsSecureFile;
static DESKTOP: WindowsDesktop = WindowsDesktop;

pub(super) struct WindowsPlatform;

impl Platform for WindowsPlatform {
    fn os_name(&self) -> &'static str {
        "windows"
    }
    fn host_kind(&self) -> HostKind {
        HostKind::Windows
    }
    fn supports_os_keystore(&self) -> bool {
        true
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

// ---- DPAPI -----------------------------------------------------------------

fn machine_scope() -> bool {
    std::env::var("TAYGEDO_DPAPI_MACHINE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn dpapi_protection() -> Protection {
    if machine_scope() {
        Protection::DpapiMachine
    } else {
        Protection::DpapiUser
    }
}

fn input_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

fn protect(key: &[u8; 32]) -> Result<Vec<u8>, PlatformError> {
    let mut flags = CRYPTPROTECT_UI_FORBIDDEN;
    if machine_scope() {
        flags |= CRYPTPROTECT_LOCAL_MACHINE;
    }
    let input = input_blob(&key[..]);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: 输入/输出指针均指向本栈帧内有效内存；out 由系统分配，
    // 失败时不会写入，成功时用 LocalFree 释放。
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
            &mut out,
        )
    };
    if ok == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(PlatformError::Unsupported(format!(
            "DPAPI CryptProtectData 失败（Win32 错误码 {code}）"
        )));
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Ok(bytes)
}

fn unprotect(blob: &[u8]) -> Result<[u8; 32], PlatformError> {
    let mut flags = CRYPTPROTECT_UI_FORBIDDEN;
    if machine_scope() {
        flags |= CRYPTPROTECT_LOCAL_MACHINE;
    }
    let input = input_blob(blob);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: 同上。
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
            &mut out,
        )
    };
    if ok == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(PlatformError::KeyCorrupt(format!(
            "DPAPI 解封失败（Win32 错误码 {code}）：密钥不属于当前账户/机器，或被篡改"
        )));
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    if bytes.len() != 32 {
        return Err(PlatformError::KeyCorrupt(format!(
            "DPAPI 解出的主密钥长度 {} 应为 32",
            bytes.len()
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    let mut bytes = bytes;
    zeroize::Zeroize::zeroize(&mut bytes);
    Ok(key)
}

struct WindowsKeyStore;

impl KeyStore for WindowsKeyStore {
    fn load_master_key(
        &self,
        data_dir: &Path,
    ) -> Result<Option<([u8; 32], Protection)>, PlatformError> {
        let path = super::keys_dir(data_dir).join(DPAPI_FILE_NAME);
        let Some(blob) = super::read_secure(&path, MAX_DPAPI_BLOB, false)? else {
            return Ok(None);
        };
        if blob.len() < 16 {
            return Err(PlatformError::KeyCorrupt(format!(
                "{} 不是合法的 DPAPI blob（长度 {}）",
                path.display(),
                blob.len()
            )));
        }
        Ok(Some((unprotect(&blob)?, dpapi_protection())))
    }

    fn store_new_master_key(
        &self,
        data_dir: &Path,
        key: &[u8; 32],
    ) -> Result<Protection, PlatformError> {
        let dir = super::keys_dir(data_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| PlatformError::Io(format!("创建密钥目录失败：{e}")))?;
        let blob = protect(key)?;
        if blob.len() > MAX_DPAPI_BLOB {
            return Err(PlatformError::Unsupported("DPAPI 输出超长".into()));
        }
        let path = dir.join(DPAPI_FILE_NAME);
        if path.exists() {
            return Err(PlatformError::KeyCorrupt(format!(
                "{} 已存在，拒绝覆盖（覆盖会使既有密文永久不可解）",
                path.display()
            )));
        }
        super::write_atomic(&path, &blob, 0o600)?;
        Ok(dpapi_protection())
    }

    fn read_pending_key(&self, _data_dir: &Path) -> Result<Option<[u8; 32]>, PlatformError> {
        Ok(None)
    }

    fn write_pending_key(&self, _data_dir: &Path, _key: &[u8; 32]) -> Result<(), PlatformError> {
        Err(PlatformError::Unsupported(
            "Windows 未实现 pending 文件轮换：请用 DPAPI 重新封装后手工替换".into(),
        ))
    }

    fn promote_pending_key(&self, _data_dir: &Path) -> Result<(), PlatformError> {
        Ok(())
    }

    fn permission_warning(&self, _keyring_path: &Path) -> Option<String> {
        // Windows 无法表达 0600 语义：重解析点校验在 SecureFile 中完成。
        None
    }
}

// ---- 安全文件 I/O ----------------------------------------------------------

struct WindowsSecureFile;

impl SecureFile for WindowsSecureFile {
    fn open_new(
        &self,
        opts: &mut OpenOptions,
        path: &Path,
        _mode: u32,
    ) -> Result<File, std::io::Error> {
        // mode 由继承的 ACL 决定；create_new(true) 由骨架设置。
        opts.open(path)
    }

    fn open_existing(&self, opts: &mut OpenOptions, path: &Path) -> Result<File, std::io::Error> {
        opts.open(path)
    }

    fn validate_metadata(
        &self,
        path: &Path,
        meta: &Metadata,
        _require_private: bool,
    ) -> Result<(), PlatformError> {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(PlatformError::Corrupt(format!(
                "{} 是重解析点（junction / 符号链接 / mount point），已拒绝",
                path.display()
            )));
        }
        Ok(())
    }

    fn fsync_dir(&self, _dir: &Path) -> std::io::Result<()> {
        // Windows 不支持对目录 fsync；sync_all 已保证文件内容落盘。
        Ok(())
    }
}

// ---- 桌面集成 --------------------------------------------------------------

struct WindowsDesktop;

impl DesktopIntegration for WindowsDesktop {
    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        // explorer 是 GUI 子系统程序：不会闪现控制台窗口，也不依赖 cmd.exe。
        // 通过 PATH 查找，不写死系统盘路径。
        let mut cmd = Command::new("explorer");
        cmd.arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.configure_command(&mut cmd);
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| PlatformError::Unsupported(format!("打开 URL 失败：{e}")))
    }

    fn configure_command(&self, cmd: &mut Command) {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    fn host_kind(&self) -> HostKind {
        HostKind::Windows
    }
}
