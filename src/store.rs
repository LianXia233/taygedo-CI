//! 文件存储：accounts.json / config.json / state.json。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::models::{Account, Config};

pub struct Store {
    pub data_dir: PathBuf,
}

impl Store {
    pub fn new(data_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&data_dir).ok();
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

    pub fn load_accounts(&self) -> Vec<Account> {
        match std::fs::read_to_string(self.accounts_path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    pub fn save_accounts(&self, accounts: &[Account]) {
        if let Ok(s) = serde_json::to_string_pretty(accounts) {
            let _ = std::fs::write(self.accounts_path(), format!("{s}\n"));
        }
    }

    pub fn load_config(&self) -> Config {
        match std::fs::read_to_string(self.config_path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    pub fn save_config(&self, config: &Config) {
        if let Ok(s) = serde_json::to_string_pretty(config) {
            let _ = std::fs::write(self.config_path(), s);
        }
    }

    pub fn load_state(&self) -> HashMap<String, String> {
        match std::fs::read_to_string(self.state_path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    pub fn save_state(&self, state: &HashMap<String, String>) {
        if let Ok(s) = serde_json::to_string_pretty(state) {
            let _ = std::fs::write(self.state_path(), s);
        }
    }
}
