//! 文件存储：accounts.json / config.json / state.json。
//!
//! 健壮性设计：
//! - 所有写入均为「临时文件 + rename」原子写，避免进程崩溃时写坏 JSON；
//! - 加载遇到损坏文件时自动备份为 `.corrupt-{时间戳}`，不再静默清空数据；
//! - 保存失败返回 `Result`，由调用方记录日志，不再无声吞掉。

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::models::{Account, Config, KeyRing};

pub struct Store {
    pub data_dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: PathBuf) -> Self {
        if let Err(e) = std::fs::create_dir_all(&data_dir) {
            eprintln!("[store] 创建数据目录失败: {} ({e})", data_dir.display());
        }
        Self { data_dir }
    }

    fn accounts_path(&self) -> PathBuf {
        self.data_dir.join("accounts.json")
    }
    fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }
    fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
    fn keyring_path(&self) -> PathBuf {
        self.data_dir.join("keyring.json")
    }

    /// 原子写：先写临时文件再 rename。返回 Err 时原文件保持不变。
    ///
    /// `mode` 为 Unix 权限位；在非 Unix 平台忽略（Windows 依靠 ACL）。
    fn atomic_write(&self, path: &std::path::Path, content: &str) -> Result<(), String> {
        self.atomic_write_mode(path, content, None)
    }

    /// 带权限控制的原子写。私钥类文件必须传 `Some(0o600)`。
    fn atomic_write_mode(
        &self,
        path: &std::path::Path,
        content: &str,
        #[cfg_attr(not(unix), allow(unused_variables))] mode: Option<u32>,
    ) -> Result<(), String> {
        let tmp = path.with_extension("json.tmp");
        let write = (|| -> std::io::Result<()> {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                if let Some(m) = mode {
                    opts.mode(m);
                }
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("写入 {} 失败: {e}", path.display()));
        }
        // 已存在的临时文件可能是旧权限，显式收紧后再 rename
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(m));
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("替换 {} 失败: {e}", path.display()));
        }
        Ok(())
    }

    /// 加载 JSON；文件不存在返回默认；解析失败时备份损坏文件并警告，不静默清空。
    fn load_json<T: serde::de::DeserializeOwned>(&self, path: &std::path::Path) -> Option<T> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => return None,
        };
        match serde_json::from_str::<T>(&raw) {
            Ok(v) => Some(v),
            Err(e) => {
                let backup = path.with_extension(format!(
                    "corrupt-{}",
                    crate::time::shanghai_datetime().replace(':', "-")
                ));
                let _ = std::fs::rename(path, &backup);
                eprintln!(
                    "[store] {} 解析失败（{e}），已备份为 {}，数据未清空",
                    path.display(),
                    backup.display()
                );
                None
            }
        }
    }

    pub fn load_accounts(&self) -> Vec<Account> {
        self.load_json(&self.accounts_path()).unwrap_or_default()
    }

    pub fn save_accounts(&self, accounts: &[Account]) -> Result<(), String> {
        let s = serde_json::to_string_pretty(accounts).map_err(|e| format!("序列化 accounts 失败: {e}"))?;
        self.atomic_write(&self.accounts_path(), &format!("{s}\n"))
    }

    pub fn load_config(&self) -> Config {
        self.load_json(&self.config_path()).unwrap_or_default()
    }

    pub fn save_config(&self, config: &Config) -> Result<(), String> {
        let s = serde_json::to_string_pretty(config).map_err(|e| format!("序列化 config 失败: {e}"))?;
        self.atomic_write(&self.config_path(), &s)
    }

    pub fn load_state(&self) -> HashMap<String, String> {
        self.load_json(&self.state_path()).unwrap_or_default()
    }

    pub fn save_state(&self, state: &HashMap<String, String>) -> Result<(), String> {
        let s = serde_json::to_string_pretty(state).map_err(|e| format!("序列化 state 失败: {e}"))?;
        self.atomic_write(&self.state_path(), &s)
    }

    /// 加载长期身份密钥环；不存在或损坏时返回 None（由调用方生成新的）。
    ///
    /// 注意：密钥环损坏**不可**静默重建后覆盖使用——重建会使已固化的客户端
    /// 指纹校验失败。因此此处只返回 None，由调用方决定是生成新密钥还是报错。
    pub fn load_keyring(&self) -> Option<KeyRing> {
        let path = self.keyring_path();
        let raw = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<KeyRing>(&raw) {
            Ok(k) => Some(k),
            Err(e) => {
                eprintln!(
                    "[store] {} 解析失败（{e}）：将保留该文件不覆盖，并使用内存临时密钥。\
                     如需重建，请手动删除该文件后重启。",
                    path.display()
                );
                None
            }
        }
    }

    /// 仅当密钥环文件不存在时写入。已存在则不覆盖（保护既有身份）。
    pub fn save_keyring_if_absent(&self, kr: &KeyRing) -> Result<bool, String> {
        let path = self.keyring_path();
        if path.exists() {
            return Ok(false);
        }
        let s = serde_json::to_string_pretty(kr).map_err(|e| format!("序列化 keyring 失败: {e}"))?;
        // 私钥文件必须以 0600 落盘
        self.atomic_write_mode(&path, &format!("{s}\n"), Some(0o600))?;
        Ok(true)
    }

    /// 强制写入密钥环（用于显式轮换）。
    /// 覆盖写入 keyring。
    ///
    /// 首次生成走 `save_keyring_if_absent`（避免并发覆盖）；
    /// 本方法是密钥轮换 / 迁移时的显式覆盖入口，由
    /// `Service::rotate_identity_key` 调用。
    #[allow(dead_code)]
    pub fn save_keyring(&self, kr: &KeyRing) -> Result<(), String> {
        let s = serde_json::to_string_pretty(kr).map_err(|e| format!("序列化 keyring 失败: {e}"))?;
        self.atomic_write_mode(&self.keyring_path(), &format!("{s}\n"), Some(0o600))
    }

    /// 校验私钥文件权限，返回诊断信息（供启动时自检）。
    #[cfg(unix)]
    pub fn keyring_permission_warning(&self) -> Option<String> {
        use std::os::unix::fs::PermissionsExt;
        let path = self.keyring_path();
        let meta = std::fs::metadata(&path).ok()?;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            Some(format!(
                "{} 权限为 {:o}，建议收紧为 600（chmod 600 {}）",
                path.display(),
                mode,
                path.display()
            ))
        } else {
            None
        }
    }

    #[cfg(not(unix))]
    pub fn keyring_permission_warning(&self) -> Option<String> {
        None
    }

    /// 数据目录路径（供文档与诊断输出）。
    /// 数据目录。
    #[allow(dead_code)]
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }
}
