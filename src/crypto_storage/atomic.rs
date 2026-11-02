//! 安全原子写入与基于文件描述符的安全读取。
//!
//! ## 威胁模型
//!
//! 数据目录可能位于可被低权限用户写入的位置（例如 OpenWrt 的 `/tmp`
//! 或某些部署中的共享目录）。因此写入路径必须抵御：
//!
//! - **符号链接替换**：临时文件用随机名 + `O_EXCL` 独占创建，跟随链接即失败；
//! - **TOCTOU**：从打开的文件描述符上做 `fstat` 校验，而不是先 `stat` 再 `open`；
//! - **截断/半写**：写入后 `fsync(file)` → `rename` → `fsync(dir)`；
//! - **权限泄漏**：创建时即以目标权限（0600）打开，并在 fd 上再收紧一次；
//! - **硬链接绕过**：读取时 `nlink != 1` 直接拒绝（存在其它链接意味着
//!   同一 inode 可能被低权限路径访问，权限模型不再成立）。
//!
//! ## 平台差异
//!
//! - Unix：`O_NOFOLLOW` + `fstat`；`rename(2)` 原子替换目录项。
//! - Windows：`create_new` + `sync_all` + `std::fs::rename`（内部为
//!   `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`）；重解析点（符号链接/junction）
//!   在读取前按文件属性拒绝。
//!
//! 注意：`rename` 覆盖**目录项**，不触碰旧 inode 的内容。SSD / flash /
//! CoW 文件系统上旧数据仍可能残留，本模块不承诺物理擦除。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rand::RngCore;

use super::StorageError;

/// 临时文件前缀。所有临时文件都以此开头，便于启动时清理残留。
pub const TEMP_PREFIX: &str = ".tgd-tmp-";

/// 生成同目录下的随机临时文件名。
fn temp_path(dir: &Path) -> PathBuf {
    let mut raw = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let mut name = String::from(TEMP_PREFIX);
    for b in raw {
        name.push_str(&format!("{b:02x}"));
    }
    dir.join(name)
}

/// 原子写入 `data` 到 `path`，权限位 `mode`（仅 Unix 生效）。
///
/// 失败时目标文件**保持原样**，临时文件被清理。
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), StorageError> {
    // `mode` 只被 Unix 分支使用；Windows 上由继承的 ACL 决定权限。
    // 显式消费一次，避免"跨平台编译时参数未使用"的告警掩盖真实问题。
    #[cfg(not(unix))]
    let _ = mode;
    let dir = path
        .parent()
        .ok_or_else(|| StorageError::Io(format!("{} 没有父目录", path.display())))?;
    let tmp = temp_path(dir);

    let write_result = (|| -> std::io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
            // O_NOFOLLOW：随机名理论上不可能被预置链接，但这一位是零成本保险。
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let mut f = opts.open(&tmp)?;
        // 在 fd 上再收紧一次：OpenOptions::mode 会被 umask 削弱。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        f.write_all(data)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(StorageError::Io(format!(
            "写入临时文件失败（{}）：{e}",
            path.display()
        )));
    }

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(StorageError::Io(format!(
            "原子替换 {} 失败：{e}",
            path.display()
        )));
    }

    // 目录项落盘。失败不致命（数据已在文件里），但必须记录，
    // 否则掉电后可能"文件内容在、目录项丢"。
    if let Err(e) = fsync_dir(dir) {
        eprintln!("[storage] 同步目录 {} 失败：{}", dir.display(), e);
    }
    Ok(())
}

/// `fsync` 目录，确保 `rename` 产生的目录项变更落盘。
pub fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let f = fs::File::open(dir)?;
        f.sync_all()
    }
    #[cfg(not(unix))]
    {
        // Windows 不支持对目录 fsync；`sync_all` 已保证文件内容落盘。
        let _ = dir;
        Ok(())
    }
}

/// 安全读取文件。
///
/// - 文件不存在 → `Ok(None)`（首次启动的正常情况）。
/// - 符号链接、非普通文件、硬链接数 > 1 → 错误。
/// - `require_private` 为真时，要求 `mode & 0o077 == 0` 且属主为当前有效用户
///   （root 运行时即要求 root 所有）。用于主密钥文件。
/// - 超过 `max_len` → `TooLarge`，不把超大文件读进内存。
pub fn read_secure(
    path: &Path,
    max_len: usize,
    require_private: bool,
) -> Result<Option<Vec<u8>>, StorageError> {
    // `require_private` 只在 Unix 上可校验（属主/权限位）。Windows 上收紧到
    // "拒绝重解析点"，无法表达 0600 语义，因此该参数不参与判定。
    #[cfg(not(unix))]
    let _ = require_private;
    // 先做一次不跟随链接的存在性判定：文件不存在是正常分支。
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(StorageError::Corrupt(format!(
                    "{} 是符号链接，已拒绝（防路径替换）",
                    path.display()
                )));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(StorageError::Io(format!(
                "读取 {} 元数据失败：{e}",
                path.display()
            )))
        }
    }

    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW：即使上面检查过后被替换成链接，这里也会 ELOOP。
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(StorageError::Io(format!(
                "打开 {} 失败：{e}",
                path.display()
            )))
        }
    };

    // 关键：校验 fd 自身（不是路径），彻底消除 TOCTOU。
    let meta = file
        .metadata()
        .map_err(|e| StorageError::Io(format!("fstat {} 失败：{e}", path.display())))?;
    if !meta.is_file() {
        return Err(StorageError::Corrupt(format!(
            "{} 不是普通文件",
            path.display()
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.nlink() != 1 {
            return Err(StorageError::Corrupt(format!(
                "{} 硬链接数为 {}（应为 1），可能被用于绕过权限保护",
                path.display(),
                meta.nlink()
            )));
        }
        if require_private {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(StorageError::MasterKeyCorrupt(format!(
                    "{} 权限为 {mode:o}，必须为 600（chmod 600 后重试）",
                    path.display()
                )));
            }
            // SAFETY: geteuid 无副作用且总是成功。
            let euid = unsafe { libc::geteuid() };
            if meta.uid() != euid {
                return Err(StorageError::MasterKeyCorrupt(format!(
                    "{} 属主 uid={} 与当前有效用户 {} 不一致",
                    path.display(),
                    meta.uid(),
                    euid
                )));
            }
        }
    }

    #[cfg(windows)]
    {
        // FILE_ATTRIBUTE_REPARSE_POINT = 0x400：junction / mount point 同样要拒绝。
        use std::os::windows::fs::MetadataExt;
        const REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & REPARSE_POINT != 0 {
            return Err(StorageError::Corrupt(format!(
                "{} 是重解析点，已拒绝",
                path.display()
            )));
        }
    }

    if meta.len() > max_len as u64 {
        return Err(StorageError::TooLarge(meta.len() as usize));
    }

    let mut buf = Vec::with_capacity(meta.len() as usize);
    // 再限一次：文件可能在 fstat 之后被追加（攻击者持有写句柄）。
    let mut limited = file.take(max_len as u64 + 1);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| StorageError::Io(format!("读取 {} 失败：{e}", path.display())))?;
    if buf.len() > max_len {
        return Err(StorageError::TooLarge(buf.len()));
    }
    Ok(Some(buf))
}

/// 清理目录中残留的临时文件（上次进程被 kill 时留下）。
///
/// 只删除本模块命名的临时文件，且只删普通文件，返回删除数量。
pub fn cleanup_stale_temp(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let path = entry.path();
        // 只删普通文件；符号链接/目录交给人工处理，避免误删。
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.is_file() && fs::remove_file(&path).is_ok() {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        write_atomic(&p, b"hello", 0o600).unwrap();
        assert_eq!(read_secure(&p, 1024, true).unwrap().unwrap(), b"hello");
    }

    #[test]
    fn read_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_secure(&dir.path().join("nope"), 1024, false)
            .unwrap()
            .is_none());
    }

    #[test]
    fn oversized_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.bin");
        write_atomic(&p, &vec![0u8; 4096], 0o600).unwrap();
        assert!(matches!(
            read_secure(&p, 1024, false),
            Err(StorageError::TooLarge(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        write_atomic(&real, b"data", 0o600).unwrap();
        let link = dir.path().join("link.bin");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(matches!(
            read_secure(&link, 1024, false),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn hardlink_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        write_atomic(&real, b"data", 0o600).unwrap();
        std::fs::hard_link(&real, dir.path().join("hard.bin")).unwrap();
        assert!(matches!(
            read_secure(&real, 1024, false),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn loose_permissions_rejected_when_private_required() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k.bin");
        write_atomic(&p, b"secret", 0o600).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            read_secure(&p, 1024, true),
            Err(StorageError::MasterKeyCorrupt(_))
        ));
        // 非私密要求下可以读，但调用方会给出告警
        assert!(read_secure(&p, 1024, false).is_ok());
    }

    #[test]
    fn temp_files_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{TEMP_PREFIX}deadbeef"));
        fs::write(&stale, b"x").unwrap();
        let keep = dir.path().join("keep.txt");
        fs::write(&keep, b"x").unwrap();
        assert_eq!(cleanup_stale_temp(dir.path()), 1);
        assert!(!stale.exists());
        assert!(keep.exists());
    }
}
