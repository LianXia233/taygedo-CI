//! 文件存储：accounts.json / config.json / state.json / keyring.json。
//!
//! ## 存储形态
//!
//! 四个文件**全部**以 XChaCha20-Poly1305 密文落盘（格式见
//! [`crate::crypto_storage::envelope`]），文件内容不再是可读 JSON。
//! 加解密完全封装在本模块内部，对外 API 的**业务语义与数据结构不变**：
//! `load_accounts()` 仍然返回 `Vec<Account>`，`save_config()` 仍然接收
//! `&Config`。
//!
//! ## 启动顺序（`Store::new`）
//!
//! 1. 创建数据目录；
//! 2. 取数据目录排他锁（防多实例并发迁移/写入）；
//! 3. 清理上次异常退出残留的临时文件；
//! 4. 扫描是否已存在密文（只看 Magic）；
//! 5. 加载或初始化主密钥（无密钥 + 有密文 → 报错，绝不覆盖）；
//! 6. 续做未完成的密钥轮换（若有）；
//! 7. 迁移旧版明文（逐文件原子操作、幂等、带加密备份）。
//!
//! ## 错误语义
//!
//! - 文件**不存在** → 返回默认值（首次启动的正常情况）。
//! - 文件存在但解密失败 / 结构非法 / 版本未知 → 返回 `Err`。
//!   **绝不**静默返回空配置，也绝不在失败时改写文件。
//!
//! 早期版本会在解析失败时把文件改名成 `.corrupt-*` 再返回默认值，这实际上
//! 等于"悄悄清空用户数据"。现在改为显式报错并保留原文件，由运维按
//! README 的恢复流程处理。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::fs_std::FileExt as _;

use crate::crypto_storage::{
    atomic, envelope, migration, FileIdentity, KeyManager, Protection, StorageCrypto, StorageError,
};
use crate::models::{Account, Config, KeyRing};

/// 数据目录锁文件名。
const LOCK_NAME: &str = ".storage.lock";

pub struct Store {
    pub data_dir: PathBuf,
    crypto: StorageCrypto,
    /// 进程生命周期内持有的排他锁。drop 时自动释放。
    _lock: std::fs::File,
    /// 启动时产生的迁移报告（供日志展示）。
    pub migration: migration::MigrationReport,
}

impl Store {
    /// 打开（或初始化）数据目录。失败即视为不可启动——上层必须终止进程，
    /// 而不是带着"没有密钥/没迁移完"的状态继续跑。
    pub fn new(data_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("创建数据目录 {} 失败：{e}", data_dir.display()))?;

        let lock = acquire_lock(&data_dir).map_err(|e| e.to_string())?;

        let cleaned = atomic::cleanup_stale_temp(&data_dir);
        if cleaned > 0 {
            eprintln!("[store] 已清理 {cleaned} 个残留临时文件");
        }

        let has_ct = migration::scan_has_ciphertext(&data_dir);
        let keys = KeyManager::load_or_init(&data_dir, has_ct).map_err(|e| e.to_string())?;
        let mut crypto = StorageCrypto::new(keys);

        // 未完成的轮换：先续做完，再迁移。
        //
        // 注意：续做成功后磁盘上的 `master.key` 已被提升为 pending 那把新密钥，
        // 而 `crypto` 里仍是**旧**密钥。必须重新装载，否则本次进程会拿旧密钥
        // 去读刚刚用新密钥加密的文件，直接 KeyIdMismatch 报错退出——
        // 那会让"中断的轮换"永远卡在启动失败上（要再重启一次才能好）。
        match migration::resume_rotation(&data_dir, &crypto) {
            Ok(None) => {}
            Ok(Some(new_id)) => {
                eprintln!("[store] 已续做未完成的密钥轮换，当前 Key ID = {new_id}");
                let keys = KeyManager::load_or_init(&data_dir, true).map_err(|e| e.to_string())?;
                crypto = StorageCrypto::new(keys);
            }
            Err(e) => return Err(format!("续做密钥轮换失败：{e}")),
        }

        let report = migration::migrate_all(&data_dir, &crypto).map_err(|e| e.to_string())?;

        Ok(Self {
            data_dir,
            crypto,
            _lock: lock,
            migration: report,
        })
    }

    fn path_of(&self, id: FileIdentity) -> PathBuf {
        self.data_dir.join(id.file_name())
    }

    /// 读取并解密一个受管文件。文件不存在返回 `Ok(None)`。
    fn read_managed(&self, id: FileIdentity) -> Result<Option<Vec<u8>>, String> {
        let path = self.path_of(id);
        let Some(blob) = atomic::read_secure(&path, crate::crypto_storage::MAX_FILE_BYTES, false)
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };

        // 明文出现在这里说明迁移被跳过（结构非法）或被外部工具写入。
        // 两种情况都不允许继续——静默当明文用等于开了一个降级后门。
        if !envelope::is_envelope(&blob) {
            return Err(format!(
                "{} 不是加密配置（旧版明文未被迁移，或文件被外部改写）。\
                 请检查该文件后重启服务以重新迁移",
                path.display()
            ));
        }
        let pt = self
            .crypto
            .open(id, &blob)
            .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        Ok(Some(pt.to_vec()))
    }

    /// 解析 JSON；空/缺失返回 `None`。
    fn load_json<T: serde::de::DeserializeOwned>(
        &self,
        id: FileIdentity,
    ) -> Result<Option<T>, String> {
        let Some(raw) = self.read_managed(id)? else {
            return Ok(None);
        };
        serde_json::from_slice::<T>(&raw).map(Some).map_err(|e| {
            format!(
                "{} 解密成功但 JSON 结构非法（{e}）；原文件未被修改",
                self.path_of(id).display()
            )
        })
    }

    /// 序列化 → 加密 → 原子写入。失败时原文件保持不变。
    fn write_managed<T: serde::Serialize>(
        &self,
        id: FileIdentity,
        value: &T,
        pretty: bool,
    ) -> Result<(), String> {
        let json = if pretty {
            serde_json::to_vec_pretty(value)
        } else {
            serde_json::to_vec(value)
        }
        .map_err(|e| format!("序列化 {} 失败：{e}", id.as_str()))?;

        let mut padded = json;
        padded.push(b'\n');

        let sealed = self.crypto.seal(id, &padded).map_err(|e| e.to_string())?;
        let path = self.path_of(id);
        atomic::write_atomic(&path, &sealed, 0o600).map_err(|e| e.to_string())?;

        // 自校验：写进去的东西必须能被自己解开，否则下一次启动就是数据丢失。
        let check = atomic::read_secure(&path, crate::crypto_storage::MAX_FILE_BYTES, false)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{} 写入后不可读", path.display()))?;
        let pt = self.crypto.open(id, &check).map_err(|e| e.to_string())?;
        if pt.as_slice() != padded.as_slice() {
            return Err(format!("{} 写入后回读校验失败", path.display()));
        }
        Ok(())
    }

    pub fn load_accounts(&self) -> Result<Vec<Account>, String> {
        Ok(self.load_json(FileIdentity::Accounts)?.unwrap_or_default())
    }

    pub fn save_accounts(&self, accounts: &[Account]) -> Result<(), String> {
        self.write_managed(FileIdentity::Accounts, &accounts, true)
    }

    pub fn load_config(&self) -> Result<Config, String> {
        Ok(self.load_json(FileIdentity::Config)?.unwrap_or_default())
    }

    pub fn save_config(&self, config: &Config) -> Result<(), String> {
        self.write_managed(FileIdentity::Config, config, true)
    }

    pub fn load_state(&self) -> Result<HashMap<String, String>, String> {
        Ok(self.load_json(FileIdentity::State)?.unwrap_or_default())
    }

    pub fn save_state(&self, state: &HashMap<String, String>) -> Result<(), String> {
        self.write_managed(FileIdentity::State, state, true)
    }

    /// 加载长期身份密钥环。
    ///
    /// - 文件不存在 → `Ok(None)`（首次启动，由调用方生成新密钥）。
    /// - 解密失败 / 结构非法 → `Err`。**不得**自动重建：重建会让已固化
    ///   服务端指纹的客户端全部校验失败，而且会悄悄换掉身份。
    pub fn load_keyring(&self) -> Result<Option<KeyRing>, String> {
        self.load_json(FileIdentity::KeyRing)
    }

    /// 仅当密钥环文件不存在时写入。已存在则不覆盖（保护既有身份）。
    pub fn save_keyring_if_absent(&self, kr: &KeyRing) -> Result<bool, String> {
        if self
            .path_of(FileIdentity::KeyRing)
            .symlink_metadata()
            .is_ok()
        {
            return Ok(false);
        }
        self.write_managed(FileIdentity::KeyRing, kr, true)?;
        Ok(true)
    }

    /// 校验密钥环文件权限，返回诊断信息（供启动时自检）。
    ///
    /// 即使内容已加密，元数据（大小、改名时间）仍可能泄漏信息，
    /// 且权限过宽意味着任意本地用户可离线爆破/替换文件。
    pub fn keyring_permission_warning(&self) -> Option<String> {
        // 权限语义属于平台能力：Unix 检查 0600，Windows 由 ACL/重解析点校验替代。
        crate::platform::current()
            .key_store()
            .permission_warning(&self.path_of(FileIdentity::KeyRing))
    }

    /// 主密钥文件/凭据的可读状态描述，用于启动日志（不含任何密钥材料）。
    pub fn key_protection(&self) -> Protection {
        self.crypto.protection()
    }

    /// 当前主密钥 ID。
    pub fn key_id_hex(&self) -> String {
        self.crypto.key_id_hex()
    }

    /// 一行存储状态摘要（写进启动日志）。
    pub fn storage_summary(&self) -> String {
        let migrated = self.migration.migrated_count();
        let prot = self.crypto.protection();
        let mut s = format!(
            "配置存储：XChaCha20-Poly1305 加密，主密钥保护={}，KeyID={}",
            prot.as_str(),
            self.crypto.key_id_hex()
        );
        if !prot.os_managed() {
            // 明确写出"这是降级方案"，避免运维误以为是硬件保护。
            s.push_str("（文件权限保护：仅限 root 可读，不等价于 TPM/DPAPI）");
        }
        if migrated > 0 {
            s.push_str(&format!("，本次迁移 {migrated} 个文件"));
        }
        if let Some(w) = self.migration.skipped().first() {
            s.push_str(&format!(
                "，{} 结构非法未迁移（原文件保留）",
                w.0.file_name()
            ));
        }
        s
    }

    /// 轮换主密钥：重新加密全部受管文件并把新密钥提升为当前密钥。
    ///
    /// 返回新的 Key ID。调用方负责在完成后重建 `AppState`（内存中的
    /// 明文数据不受影响，但后续写入必须使用新密钥）。
    pub fn rotate_storage_key(&mut self) -> Result<String, String> {
        let old_id = self.crypto.key_id_hex();
        let mut master = zeroize::Zeroizing::new([0u8; 32]);
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut master[..]);
        let new_keys = KeyManager::from_master(master, self.crypto.protection());
        let report = migration::rotate_key(&self.data_dir, &self.crypto, new_keys)
            .map_err(|e| e.to_string())?;

        // 重新加载，使内存中的 crypto 持有新密钥。
        let has_ct = migration::scan_has_ciphertext(&self.data_dir);
        let keys = KeyManager::load_or_init(&self.data_dir, has_ct).map_err(|e| e.to_string())?;
        self.crypto = StorageCrypto::new(keys);

        // 报告与实况必须一致，否则说明轮换只做了一半。
        if report.old_key_id != old_id {
            return Err(format!(
                "轮换期间主密钥发生了变化（预期 {old_id}，实际 {}），请检查数据目录",
                report.old_key_id
            ));
        }
        if report.new_key_id != self.crypto.key_id_hex() {
            return Err("轮换后重新加载的主密钥与预期不一致".to_string());
        }
        Ok(report.new_key_id)
    }

    /// 数据目录路径（供文档与诊断输出）。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// 取数据目录排他锁。最多等待 10 秒，超时报错（另一个实例正在运行）。
///
/// 排他锁的作用不是"防止别人读"，而是防止**两个实例同时迁移/写入**
/// 造成文件互相覆盖——那是真正会丢数据的场景。
fn acquire_lock(data_dir: &Path) -> Result<std::fs::File, StorageError> {
    let path = data_dir.join(LOCK_NAME);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| StorageError::Io(format!("打开锁文件 {} 失败：{e}", path.display())))?;

    for _ in 0..50 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    Err(StorageError::Locked(format!(
        "{} 已被另一个 taygedo 实例锁定（等待 10 秒超时）。请先停止其它实例后重试",
        data_dir.display()
    )))
}

#[cfg(test)]
mod tests;
