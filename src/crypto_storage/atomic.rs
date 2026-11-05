//! 安全原子写入与读取的**薄包装**。
//!
//! 跨平台骨架与平台相关钩子全部位于 [`crate::platform::fs`]：
//! Unix 用 `O_NOFOLLOW` + `fstat` + `nlink`/`euid` 校验，Windows 用
//! `create_new` + 重解析点校验。本模块只做错误类型映射，保持原有调用点
//! 与签名不变，避免把平台判断重新渗回业务代码。

use std::path::Path;

use super::StorageError;
use crate::platform as sys;

// 兼容既有调用点：部分形态只用到其中一部分导出。
#[allow(unused_imports)]
pub use crate::platform::fs::TEMP_PREFIX;

/// 原子写入 `data` 到 `path`，权限位 `mode`（仅 Unix 生效）。
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), StorageError> {
    sys::current()
        .secure_file()
        .write_atomic(path, data, mode)
        .map_err(StorageError::from)
}

/// `fsync` 目录，确保 `rename` 产生的目录项变更落盘。
#[allow(dead_code)]
pub fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    sys::fsync_dir(dir)
}

/// 安全读取文件。符号链接、非普通文件、硬链接、权限不安全一律拒绝。
pub fn read_secure(
    path: &Path,
    max_len: usize,
    require_private: bool,
) -> Result<Option<Vec<u8>>, StorageError> {
    sys::current()
        .secure_file()
        .read_secure(path, max_len, require_private)
        .map_err(StorageError::from)
}

/// 清理目录中残留的临时文件（上次进程被 kill 时留下）。
pub fn cleanup_stale_temp(dir: &Path) -> usize {
    sys::cleanup_stale_temp(dir)
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

    #[test]
    fn temp_files_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{TEMP_PREFIX}deadbeef"));
        std::fs::write(&stale, b"x").unwrap();
        let keep = dir.path().join("keep.txt");
        std::fs::write(&keep, b"x").unwrap();
        assert_eq!(cleanup_stale_temp(dir.path()), 1);
        assert!(!stale.exists());
        assert!(keep.exists());
    }
}
