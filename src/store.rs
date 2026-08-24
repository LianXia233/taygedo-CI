//! 文件存储：accounts.json / config.json / state.json。
//!
//! 健壮性设计：
//! - 所有写入均为「临时文件 + rename」原子写，避免进程崩溃时写坏 JSON；
//! - 加载遇到损坏文件时自动备份为 `.corrupt-{时间戳}`，不再静默清空数据；
//! - 保存失败返回 `Result`，由调用方记录日志，不再无声吞掉。

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::models::{Account, Config};

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

    /// 原子写：先写临时文件再 rename。返回 Err 时原文件保持不变。
    fn atomic_write(&self, path: &std::path::Path, content: &str) -> Result<(), String> {
        let tmp = path.with_extension("json.tmp");
        let write = (|| -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("写入 {} 失败: {e}", path.display()));
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
}
