//! 安全文件 I/O 骨架：平台无关流程 + 平台相关钩子。
//!
//! 骨架负责「随机临时名 → 独占创建 → fsync → rename → fsync(dir)」以及
//! 「不跟随链接的存在性判定 → 在 fd 上校验元数据 → 长度上限」；
//! 平台实现只提供三个钩子（打开方式、fd 元数据校验、目录 fsync）。
//! 因此 Unix 的 `O_NOFOLLOW`/`nlink`/`euid` 与 Windows 的重解析点校验
//! 各自位于 `platform/unix.rs` / `platform/windows.rs`，业务代码零 `cfg`。

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rand::RngCore;

use super::PlatformError;

pub const TEMP_PREFIX: &str = ".tgd-tmp-";

fn temp_path(dir: &Path) -> PathBuf {
    let mut raw = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let mut name = String::from(TEMP_PREFIX);
    for b in raw {
        name.push_str(&format!("{b:02x}"));
    }
    dir.join(name)
}

/// 平台相关的文件安全钩子。默认方法提供跨平台骨架。
pub trait SecureFile: Send + Sync {
    /// 以独占方式创建临时文件（Unix 需附加 mode 与 O_NOFOLLOW 等标志）。
    fn open_new(
        &self,
        opts: &mut OpenOptions,
        path: &Path,
        mode: u32,
    ) -> Result<File, std::io::Error>;

    /// 以只读方式打开既有文件（Unix 需附加 O_NOFOLLOW / O_CLOEXEC）。
    fn open_existing(&self, opts: &mut OpenOptions, path: &Path) -> Result<File, std::io::Error>;

    /// 在已打开文件的 fd 元数据上做平台安全校验（消除 TOCTOU）。
    fn validate_metadata(
        &self,
        path: &Path,
        meta: &Metadata,
        require_private: bool,
    ) -> Result<(), PlatformError>;

    /// 目录项落盘（Windows 无对应能力，为空操作）。
    fn fsync_dir(&self, dir: &Path) -> std::io::Result<()>;

    /// 原子写入。失败时目标文件保持原样，临时文件被清理。
    fn write_atomic(&self, path: &Path, data: &[u8], mode: u32) -> Result<(), PlatformError> {
        let dir = path
            .parent()
            .ok_or_else(|| PlatformError::Io(format!("{} 没有父目录", path.display())))?;
        let tmp = temp_path(dir);
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);

        let result = (|| -> Result<(), PlatformError> {
            let mut f = self.open_new(&mut opts, &tmp, mode)?;
            f.write_all(data)
                .map_err(|e| PlatformError::Io(format!("写入临时文件失败：{e}")))?;
            f.sync_all()
                .map_err(|e| PlatformError::Io(format!("同步临时文件失败：{e}")))?;
            Ok(())
        })();

        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }

        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(PlatformError::Io(format!(
                "原子替换 {} 失败：{e}",
                path.display()
            )));
        }
        if let Err(e) = self.fsync_dir(dir) {
            eprintln!("[storage] 同步目录 {} 失败：{}", dir.display(), e);
        }
        Ok(())
    }

    /// 安全读取。`Ok(None)` 表示文件不存在；链接/权限/长度异常一律报错。
    fn read_secure(
        &self,
        path: &Path,
        max_len: usize,
        require_private: bool,
    ) -> Result<Option<Vec<u8>>, PlatformError> {
        match fs::symlink_metadata(path) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(PlatformError::Corrupt(format!(
                        "{} 是符号链接，已拒绝（防路径替换）",
                        path.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(PlatformError::Io(format!(
                    "读取 {} 元数据失败：{e}",
                    path.display()
                )))
            }
        }

        let mut opts = OpenOptions::new();
        opts.read(true);
        let file = match self.open_existing(&mut opts, path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(PlatformError::Io(format!(
                    "打开 {} 失败：{e}",
                    path.display()
                )))
            }
        };

        let meta = file
            .metadata()
            .map_err(|e| PlatformError::Io(format!("fstat {} 失败：{e}", path.display())))?;
        if !meta.is_file() {
            return Err(PlatformError::Corrupt(format!(
                "{} 不是普通文件",
                path.display()
            )));
        }
        self.validate_metadata(path, &meta, require_private)?;

        if meta.len() > max_len as u64 {
            return Err(PlatformError::TooLarge(meta.len() as usize));
        }
        let mut buf = Vec::with_capacity(meta.len() as usize);
        let mut limited = file.take(max_len as u64 + 1);
        limited
            .read_to_end(&mut buf)
            .map_err(|e| PlatformError::Io(format!("读取 {} 失败：{e}", path.display())))?;
        if buf.len() > max_len {
            return Err(PlatformError::TooLarge(buf.len()));
        }
        Ok(Some(buf))
    }

    /// 清理上次进程被 kill 时残留的临时文件（只删本模块命名的普通文件）。
    fn cleanup_stale_temp(&self, dir: &Path) -> usize {
        let mut n = 0;
        let Ok(rd) = fs::read_dir(dir) else { return 0 };
        for entry in rd.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let path = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&path) {
                if meta.is_file() && fs::remove_file(&path).is_ok() {
                    n += 1;
                }
            }
        }
        n
    }
}
