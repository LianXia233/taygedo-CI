//! Unix 家族平台实现：systemd 凭据 / 0600 密钥文件 / fd 级安全校验。
//!
//! Linux、OpenWrt、macOS 共用本实现：
//! - 密钥来源按宿主能力选择（systemd 凭据 → 0600 文件）；
//! - 文件安全依赖 `O_NOFOLLOW` + `fstat` + `nlink` + `euid`；
//! - macOS 复用 Unix 权限模型，仅在打开 URL 时改用 `open`。
//!
//! 宿主类型可用 `TAYGEDO_HOST_KIND=openwrt|linux|macos` 显式声明，
//! 避免在容器/交叉环境中依赖固件标记文件。

use std::fs::{File, Metadata, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use base64::Engine as _;

use super::fs::SecureFile;
use super::{
    decode_key_blob, encode_key_blob, keys_dir, DesktopIntegration, HostKind, KeyStore, Platform,
    PlatformError, Protection, CREDENTIAL_NAME, KEY_BLOB_LEN, KEY_FILE_NAME, PENDING_KEY_FILE_NAME,
};

pub(super) static PLATFORM: UnixPlatform = UnixPlatform;
static KEY_STORE: UnixKeyStore = UnixKeyStore;
static SECURE_FILE: UnixSecureFile = UnixSecureFile;
static DESKTOP: UnixDesktop = UnixDesktop;

pub(super) struct UnixPlatform;

impl Platform for UnixPlatform {
    fn os_name(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "macos"
        } else {
            "unix"
        }
    }
    fn host_kind(&self) -> HostKind {
        DESKTOP.host_kind()
    }
    fn supports_os_keystore(&self) -> bool {
        credentials_dir().is_some()
    }
    fn supports_key_rotation(&self) -> bool {
        true
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

fn host_override() -> Option<String> {
    std::env::var("TAYGEDO_HOST_KIND")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
}

/// 宿主是否为 OpenWrt（运行期探测，与构建形态无关）。
fn is_openwrt_host() -> bool {
    match host_override() {
        Some(v) => v == "openwrt",
        None => Path::new("/etc/openwrt_release").exists(),
    }
}

fn host_protection() -> Protection {
    if is_openwrt_host() {
        Protection::OpenWrtFile
    } else {
        Protection::UnixFile
    }
}

fn credentials_dir() -> Option<PathBuf> {
    let v = std::env::var_os("CREDENTIALS_DIRECTORY")?;
    if v.is_empty() {
        return None;
    }
    Some(PathBuf::from(v))
}

/// 解析 systemd 凭据内容：32 字节裸数据 / 64 位 hex / base64。
fn decode_credential(raw: &[u8]) -> Result<[u8; 32], PlatformError> {
    let mut end = raw.len();
    while end > 0 && matches!(raw[end - 1], b'\n' | b'\r' | b' ') {
        end -= 1;
    }
    let trimmed = &raw[..end];
    let mut out = [0u8; 32];
    if trimmed.len() == 32 {
        out.copy_from_slice(trimmed);
        return Ok(out);
    }
    if trimmed.len() == 64 && trimmed.iter().all(|b| b.is_ascii_hexdigit()) {
        for i in 0..32 {
            let hi = hex_val(trimmed[i * 2])?;
            let lo = hex_val(trimmed[i * 2 + 1])?;
            out[i] = (hi << 4) | lo;
        }
        return Ok(out);
    }
    for eng in [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ] {
        if let Ok(decoded) = eng.decode(trimmed) {
            if decoded.len() == 32 {
                out.copy_from_slice(&decoded);
                return Ok(out);
            }
        }
    }
    Err(PlatformError::KeyCorrupt(format!(
        "systemd 凭据 {} 内容非法：期望 32 字节裸数据 / 64 位十六进制 / base64，实际 {} 字节",
        CREDENTIAL_NAME,
        trimmed.len()
    )))
}

fn hex_val(b: u8) -> Result<u8, PlatformError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(PlatformError::KeyCorrupt("十六进制字符非法".into())),
    }
}

fn credential_key() -> Result<Option<[u8; 32]>, PlatformError> {
    let Some(dir) = credentials_dir() else {
        return Ok(None);
    };
    let path = dir.join(CREDENTIAL_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read(&path).map_err(|e| {
        PlatformError::KeyCorrupt(format!("读取 systemd 凭据 {} 失败：{e}", path.display()))
    })?;
    decode_credential(&raw).map(Some)
}

struct UnixKeyStore;

impl KeyStore for UnixKeyStore {
    fn load_master_key(
        &self,
        data_dir: &Path,
    ) -> Result<Option<([u8; 32], Protection)>, PlatformError> {
        if let Some(key) = credential_key()? {
            return Ok(Some((key, Protection::SystemdCredential)));
        }
        let path = keys_dir(data_dir).join(KEY_FILE_NAME);
        let Some(blob) = super::read_secure(&path, KEY_BLOB_LEN, true)? else {
            return Ok(None);
        };
        Ok(Some((decode_key_blob(&blob)?, host_protection())))
    }

    fn store_new_master_key(
        &self,
        data_dir: &Path,
        key: &[u8; 32],
    ) -> Result<Protection, PlatformError> {
        let dir = keys_dir(data_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| PlatformError::Io(format!("创建密钥目录失败：{e}")))?;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let protection = host_protection();
        let blob = encode_key_blob(key, protection);
        let path = dir.join(KEY_FILE_NAME);
        if path.exists() {
            return Err(PlatformError::KeyCorrupt(format!(
                "{} 已存在，拒绝覆盖（覆盖会使既有密文永久不可解）",
                path.display()
            )));
        }
        super::write_atomic(&path, &blob, 0o600)?;
        Ok(protection)
    }

    fn read_pending_key(&self, data_dir: &Path) -> Result<Option<[u8; 32]>, PlatformError> {
        let path = keys_dir(data_dir).join(PENDING_KEY_FILE_NAME);
        let Some(blob) = super::read_secure(&path, KEY_BLOB_LEN, true)? else {
            return Ok(None);
        };
        Ok(Some(decode_key_blob(&blob)?))
    }

    fn write_pending_key(&self, data_dir: &Path, key: &[u8; 32]) -> Result<(), PlatformError> {
        let dir = keys_dir(data_dir);
        std::fs::create_dir_all(&dir)
            .map_err(|e| PlatformError::Io(format!("创建密钥目录失败：{e}")))?;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let blob = encode_key_blob(key, host_protection());
        let path = dir.join(PENDING_KEY_FILE_NAME);
        // pending 是过渡产物，覆盖不会让既有密文不可解。
        super::write_atomic(&path, &blob, 0o600)
    }

    fn promote_pending_key(&self, data_dir: &Path) -> Result<(), PlatformError> {
        let dir = keys_dir(data_dir);
        std::fs::rename(dir.join(PENDING_KEY_FILE_NAME), dir.join(KEY_FILE_NAME))
            .map_err(|e| PlatformError::Io(format!("提升待用主密钥失败：{e}")))?;
        super::fsync_dir(&dir).ok();
        Ok(())
    }

    fn permission_warning(&self, keyring_path: &Path) -> Option<String> {
        let meta = std::fs::symlink_metadata(keyring_path).ok()?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            Some(format!(
                "{} 权限为 {mode:o}，建议收紧为 600",
                keyring_path.display()
            ))
        } else {
            None
        }
    }
}

struct UnixSecureFile;

impl SecureFile for UnixSecureFile {
    fn open_new(
        &self,
        opts: &mut OpenOptions,
        path: &Path,
        mode: u32,
    ) -> Result<File, std::io::Error> {
        opts.mode(mode);
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let f = opts.open(path)?;
        // OpenOptions::mode 会被 umask 削弱，在 fd 上再收紧一次。
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        Ok(f)
    }

    fn open_existing(&self, opts: &mut OpenOptions, path: &Path) -> Result<File, std::io::Error> {
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        opts.open(path)
    }

    fn validate_metadata(
        &self,
        path: &Path,
        meta: &Metadata,
        require_private: bool,
    ) -> Result<(), PlatformError> {
        if meta.nlink() != 1 {
            return Err(PlatformError::Corrupt(format!(
                "{} 硬链接数为 {}（应为 1），可能被用于绕过权限保护",
                path.display(),
                meta.nlink()
            )));
        }
        if require_private {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(PlatformError::KeyCorrupt(format!(
                    "{} 权限为 {mode:o}，必须为 600（chmod 600 后重试）",
                    path.display()
                )));
            }
            // SAFETY: geteuid 无副作用且总是成功。
            let euid = unsafe { libc::geteuid() };
            if meta.uid() != euid {
                return Err(PlatformError::KeyCorrupt(format!(
                    "{} 属主 uid={} 与当前有效用户 {} 不一致",
                    path.display(),
                    meta.uid(),
                    euid
                )));
            }
        }
        Ok(())
    }

    fn fsync_dir(&self, dir: &Path) -> std::io::Result<()> {
        File::open(dir)?.sync_all()
    }
}

struct UnixDesktop;

impl DesktopIntegration for UnixDesktop {
    fn host_kind(&self) -> HostKind {
        if let Some(v) = host_override() {
            return match v.as_str() {
                "openwrt" => HostKind::OpenWrt,
                "linux" => HostKind::Linux,
                "macos" => HostKind::MacOS,
                _ => HostKind::Other,
            };
        }
        if cfg!(target_os = "macos") {
            HostKind::MacOS
        } else if is_openwrt_host() {
            HostKind::OpenWrt
        } else {
            HostKind::Linux
        }
    }

    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        // 允许运维显式指定程序（TAYGEDO_BROWSER），否则按宿主选择标准工具。
        let program = std::env::var("TAYGEDO_BROWSER")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                if matches!(self.host_kind(), HostKind::MacOS) {
                    "open".to_string()
                } else {
                    "xdg-open".to_string()
                }
            });
        let mut cmd = Command::new(&program);
        cmd.arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.configure_command(&mut cmd);
        cmd.spawn()
            .map(|_| ())
            .map_err(|e| PlatformError::Unsupported(format!("{program} 打开 URL 失败：{e}")))
    }

    fn configure_command(&self, _cmd: &mut Command) {
        // Unix 无需额外进程标志。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_raw_hex_and_base64() {
        let key = [0xABu8; 32];
        assert_eq!(decode_credential(&key).unwrap(), key);
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(decode_credential(hex.as_bytes()).unwrap(), key);
        let hex_nl = format!("{hex}\n");
        assert_eq!(decode_credential(hex_nl.as_bytes()).unwrap(), key);
        let b64 = base64::engine::general_purpose::STANDARD.encode(key);
        assert_eq!(decode_credential(b64.as_bytes()).unwrap(), key);
    }

    #[test]
    fn decode_rejects_wrong_length() {
        assert!(decode_credential(b"short").is_err());
        assert!(decode_credential(&[0u8; 31]).is_err());
        assert!(decode_credential(&[b'z'; 64]).is_err());
    }

    #[test]
    fn symlink_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        super::super::write_atomic(&real, b"data", 0o600).unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            super::super::read_secure(&link, 1024, false),
            Err(PlatformError::Corrupt(_))
        ));
    }

    #[test]
    fn hardlink_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        super::super::write_atomic(&real, b"data", 0o600).unwrap();
        std::fs::hard_link(&real, dir.path().join("hard.bin")).unwrap();
        assert!(matches!(
            super::super::read_secure(&real, 1024, false),
            Err(PlatformError::Corrupt(_))
        ));
    }

    #[test]
    fn loose_permissions_rejected_when_private_required() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.bin");
        super::super::write_atomic(&p, b"secret", 0o600).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            super::super::read_secure(&p, 1024, true),
            Err(PlatformError::KeyCorrupt(_))
        ));
        assert!(super::super::read_secure(&p, 1024, false).is_ok());
    }
}
