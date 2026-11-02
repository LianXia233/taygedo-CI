//! 旧版明文自动迁移、加密备份与密钥轮换。
//!
//! ## 迁移状态机
//!
//! 对每个受管文件独立执行（幂等）：
//!
//! ```text
//!                 ┌──────────────┐
//!   文件不存在 ──▶│    absent    │ 什么都不做
//!                 └──────────────┘
//!   首字节 = Magic ──▶ 尝试用当前主密钥解密 ──成功──▶ already_encrypted
//!                                    └─失败──▶ 报错（密钥不符/被篡改，绝不覆盖）
//!   其它内容 ──▶ 按 UTF-8 + JSON 解析校验
//!                 ├─ 失败 ──▶ skipped_invalid（保留原文件，不迁移，不删除）
//!                 └─ 成功 ──▶ seal → 写加密备份 → 校验备份 → 原子替换 → 回读校验
//!                                      └─ 任何一步失败 ──▶ 原文件保持不变
//! ```
//!
//! ### 为什么可以"中断后恢复"
//!
//! 每个文件的迁移都是"临时文件 + rename"的原子操作，且**幂等**：
//! 中断只会留下两种状态——原明文（下次重新迁移）或已加密（下次跳过）。
//! 不需要额外的两阶段提交。
//!
//! ### 明文备份的策略
//!
//! 迁移过程中**不产生任何明文备份**。备份同样是加密文件（同一身份、同一
//! 主密钥），因此：
//! - 优点：磁盘上不存在"旧版明文副本"这一长期泄漏面；
//! - 代价：主密钥丢失时备份与主文件一起不可解 —— 这正是要求"主密钥
//!   必须单独备份"的原因（见 README 的恢复流程）。
//!
//! 注意：`rename` 只替换目录项。SSD / flash / CoW 文件系统上旧明文块
//! 可能残留，**不承诺**通过 `unlink` 实现物理擦除。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::atomic;
use super::key_manager::KeyManager;
use super::{envelope, FileIdentity, StorageCrypto, StorageError, MAX_FILE_BYTES};

/// 迁移日志文件名（**仅含非敏感元数据**：版本、时间、Key ID、状态）。
pub const JOURNAL_NAME: &str = ".storage-migration.json";
/// 迁移日志版本。
pub const JOURNAL_VERSION: u8 = 1;
/// 每个文件保留的备份数量上限。
pub const BACKUP_KEEP: usize = 3;

/// 单个文件的迁移结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOutcome {
    /// 文件不存在，跳过。
    Absent,
    /// 原本就是密文（已迁移过），未改动。
    AlreadyEncrypted,
    /// 刚刚从明文迁移为密文。
    Migrated,
    /// 明文结构非法，保留原样未迁移。
    SkippedInvalid,
}

impl FileOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            FileOutcome::Absent => "absent",
            FileOutcome::AlreadyEncrypted => "encrypted",
            FileOutcome::Migrated => "migrated",
            FileOutcome::SkippedInvalid => "skipped_invalid",
        }
    }
}

/// 迁移报告，用于启动日志。
#[derive(Debug, Default)]
pub struct MigrationReport {
    pub outcomes: Vec<(FileIdentity, FileOutcome)>,
    pub backup_paths: Vec<PathBuf>,
}

impl MigrationReport {
    pub fn migrated_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|(_, o)| *o == FileOutcome::Migrated)
            .count()
    }
    pub fn skipped(&self) -> Vec<(FileIdentity, FileOutcome)> {
        self.outcomes
            .iter()
            .filter(|(_, o)| *o == FileOutcome::SkippedInvalid)
            .cloned()
            .collect()
    }
}

/// 迁移日志条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// 状态字符串（与 [`FileOutcome::as_str`] 一致，另含 `rotate_bak`）。
    pub state: String,
    /// 记录时间（上海时区可读串）。
    pub at: String,
    /// 关联备份文件名（若有）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

/// 迁移日志。放在数据目录根下，0600，可人工查看。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub journal_version: u8,
    pub storage_format_version: u8,
    /// 写入时的主密钥 ID（用于运维判断"这批文件是哪代密钥加的"）。
    pub key_id: String,
    pub updated_at: String,
    pub files: BTreeMap<String, JournalEntry>,
}

impl Journal {
    fn new(key_id: &str) -> Self {
        Self {
            journal_version: JOURNAL_VERSION,
            storage_format_version: super::FORMAT_VERSION,
            key_id: key_id.to_string(),
            updated_at: crate::time::shanghai_datetime(),
            files: BTreeMap::new(),
        }
    }

    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(JOURNAL_NAME)
    }

    /// 读取日志；损坏或版本不符时返回空日志（日志本身不承载数据，丢了不影响安全性）。
    pub fn load(data_dir: &Path) -> Self {
        let Ok(Some(raw)) = atomic::read_secure(&Self::path(data_dir), 256 * 1024, false) else {
            return Self::new("");
        };
        match serde_json::from_slice::<Journal>(&raw) {
            Ok(j) if j.journal_version == JOURNAL_VERSION => j,
            Ok(j) => {
                eprintln!(
                    "[storage] 迁移日志版本 {} 不受支持，按空日志处理",
                    j.journal_version
                );
                Self::new("")
            }
            Err(e) => {
                eprintln!("[storage] 迁移日志损坏（{e}），按空日志处理");
                Self::new("")
            }
        }
    }

    fn save(&mut self, data_dir: &Path) {
        self.updated_at = crate::time::shanghai_datetime();
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(e) = atomic::write_atomic(&Self::path(data_dir), &bytes, 0o600) {
                    eprintln!("[storage] 写入迁移日志失败：{e}");
                }
            }
            Err(e) => eprintln!("[storage] 序列化迁移日志失败：{e}"),
        }
    }

    fn record(&mut self, data_dir: &Path, id: FileIdentity, state: &str, backup: Option<&Path>) {
        self.files.insert(
            id.as_str().to_string(),
            JournalEntry {
                state: state.to_string(),
                at: crate::time::shanghai_datetime(),
                backup: backup.and_then(|p| p.file_name().map(|n| n.to_string_lossy().into())),
            },
        );
        self.save(data_dir);
    }
}

/// 扫描数据目录，判断是否已存在**任意**密文文件。
///
/// 只读每个文件的前 8 字节（不解析、不解密），因此可以在拿到主密钥之前调用。
pub fn scan_has_ciphertext(data_dir: &Path) -> bool {
    for id in FileIdentity::ALL {
        let path = data_dir.join(id.file_name());
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() < envelope::MAGIC.len() as u64 {
            continue;
        }
        let Ok(mut f) = std::fs::File::open(&path) else {
            continue;
        };
        let mut head = [0u8; 8];
        use std::io::Read;
        if f.read_exact(&mut head).is_ok() && head == envelope::MAGIC {
            return true;
        }
    }
    false
}

/// 时间戳片段（用于备份文件名，去掉 Windows 不允许的字符）。
fn ts_tag() -> String {
    crate::time::shanghai_datetime()
        .replace(':', "")
        .replace(' ', "-")
        .replace('/', "")
}

/// 备份文件名：`<原名>.enc-bak-<时间戳>`。
pub fn backup_name(id: FileIdentity, tag: &str) -> String {
    format!("{}.enc-bak-{tag}", id.file_name())
}

/// 列出某文件的全部备份（按文件名倒序，最新在前）。
pub fn list_backups(data_dir: &Path, id: FileIdentity) -> Vec<PathBuf> {
    let prefix = format!("{}.enc-bak-", id.file_name());
    let mut out: Vec<PathBuf> = std::fs::read_dir(data_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    out.reverse();
    out
}

/// 按保留策略清理旧备份，只保留最新 `BACKUP_KEEP` 份。
fn prune_backups(data_dir: &Path, id: FileIdentity) {
    for old in list_backups(data_dir, id).into_iter().skip(BACKUP_KEEP) {
        if let Err(e) = std::fs::remove_file(&old) {
            eprintln!("[storage] 清理旧备份 {} 失败：{e}", old.display());
        }
    }
}

/// 用给定明文校验其结构是否符合该文件应有的模型。
///
/// 迁移前必须做这一步：如果旧文件其实不是合法 JSON，加密它就等于把
/// "本来就坏的数据"封进密文，之后再也无法人工抢救。
fn validate_plaintext(id: FileIdentity, raw: &[u8]) -> Result<(), StorageError> {
    let bad = |m: String| StorageError::InvalidPlaintext(m);
    let text = std::str::from_utf8(raw).map_err(|_| bad("不是合法 UTF-8".into()))?;
    if text.trim().is_empty() {
        return Err(bad("内容为空".into()));
    }
    match id {
        FileIdentity::Accounts => {
            let v: Vec<crate::models::Account> =
                serde_json::from_str(text).map_err(|e| bad(format!("accounts 结构非法：{e}")))?;
            for a in &v {
                if a.id.trim().is_empty() {
                    return Err(bad("存在 id 为空的账号记录".into()));
                }
            }
        }
        FileIdentity::Config => {
            let c: crate::models::Config =
                serde_json::from_str(text).map_err(|e| bad(format!("config 结构非法：{e}")))?;
            if c.credential_key.is_empty() {
                return Err(bad("config.credential_key 为空".into()));
            }
        }
        FileIdentity::State => {
            let _: std::collections::HashMap<String, String> =
                serde_json::from_str(text).map_err(|e| bad(format!("state 结构非法：{e}")))?;
        }
        FileIdentity::KeyRing => {
            let k: crate::models::KeyRing =
                serde_json::from_str(text).map_err(|e| bad(format!("keyring 结构非法：{e}")))?;
            k.secret_bytes().map_err(bad)?;
        }
    }
    Ok(())
}

/// 生成加密备份并回读校验。返回备份路径。
fn write_backup(
    data_dir: &Path,
    crypto: &StorageCrypto,
    id: FileIdentity,
    plaintext: &[u8],
) -> Result<PathBuf, StorageError> {
    let sealed = crypto.seal(id, plaintext)?;
    let path = data_dir.join(backup_name(id, &ts_tag()));
    atomic::write_atomic(&path, &sealed, 0o600)?;
    // 备份必须自己也能解开，否则"有备份"是假的。
    let back = atomic::read_secure(&path, MAX_FILE_BYTES, false)?
        .ok_or_else(|| StorageError::Io("备份写入后不可读".into()))?;
    let pt = crypto.open(id, &back)?;
    if pt.as_slice() != plaintext {
        return Err(StorageError::Corrupt("备份回读内容与原始明文不一致".into()));
    }
    Ok(path)
}

/// 对单个文件执行迁移。
fn migrate_one(
    data_dir: &Path,
    crypto: &StorageCrypto,
    id: FileIdentity,
    report: &mut MigrationReport,
    journal: &mut Journal,
) -> Result<FileOutcome, StorageError> {
    let path = data_dir.join(id.file_name());
    let Some(blob) = atomic::read_secure(&path, MAX_FILE_BYTES, false)? else {
        return Ok(FileOutcome::Absent);
    };

    if envelope::is_envelope(&blob) {
        // 已加密：必须能解开才算真的"已迁移"，否则说明主密钥不对或文件被换过。
        crypto.open(id, &blob)?;
        return Ok(FileOutcome::AlreadyEncrypted);
    }

    // 旧版明文
    if let Err(reason) = validate_plaintext(id, &blob) {
        eprintln!(
            "[storage] {} 不是合法旧版配置（{reason}），保留原文件不迁移",
            path.display()
        );
        return Ok(FileOutcome::SkippedInvalid);
    }

    let sealed = crypto.seal(id, &blob)?;

    // 1) 备份（加密）并校验
    let backup = write_backup(data_dir, crypto, id, &blob)?;
    report.backup_paths.push(backup.clone());

    // 2) 原子替换主文件
    atomic::write_atomic(&path, &sealed, 0o600)?;

    // 3) 回读校验：认证标签 + 内容一致
    let check = atomic::read_secure(&path, MAX_FILE_BYTES, false)?
        .ok_or_else(|| StorageError::Io("迁移后文件不可读".into()))?;
    let pt = crypto.open(id, &check)?;
    if pt.as_slice() != blob.as_slice() {
        return Err(StorageError::Corrupt(format!(
            "{} 迁移后内容与原明文不一致（已保留加密备份 {}）",
            path.display(),
            backup.display()
        )));
    }

    prune_backups(data_dir, id);
    journal.record(data_dir, id, FileOutcome::Migrated.as_str(), Some(&backup));
    Ok(FileOutcome::Migrated)
}

/// 迁移数据目录下的全部旧版明文配置。
///
/// 幂等：已加密的文件只做"能否解开"的校验，不重写。
pub fn migrate_all(
    data_dir: &Path,
    crypto: &StorageCrypto,
) -> Result<MigrationReport, StorageError> {
    let mut report = MigrationReport::default();
    let mut journal = Journal::load(data_dir);
    journal.key_id = crypto.key_id_hex();
    journal.storage_format_version = super::FORMAT_VERSION;

    for id in FileIdentity::ALL {
        let outcome = migrate_one(data_dir, crypto, id, &mut report, &mut journal)?;
        if outcome != FileOutcome::Absent {
            journal.record(data_dir, id, outcome.as_str(), None);
        }
        report.outcomes.push((id, outcome));
    }
    journal.save(data_dir);
    Ok(report)
}

// ---------------------------------------------------------------------------
// 密钥轮换
// ---------------------------------------------------------------------------

/// 轮换结果。
#[derive(Debug, Default)]
pub struct RotationReport {
    pub reencrypted: Vec<FileIdentity>,
    pub old_key_id: String,
    pub new_key_id: String,
}

/// 用新主密钥重新加密全部受管文件，然后提升新密钥。
///
/// ## 顺序
///
/// 1. 新密钥先写入 `keys/master.key.pending`（**先保证新密钥不丢**）；
/// 2. 逐文件"用旧密钥解密 → 用新密钥加密 → 原子替换"，旧密文留一份
///    `.rotate-bak-<旧KeyID>` 备份；
/// 3. 全部成功后原子把 pending 提升为 `master.key`。
///
/// 任一步中断，下次启动时 [`resume_rotation`] 会接着做完：仍能用旧密钥
/// 解开的文件继续轮换，已经能用新密钥解开的直接跳过。
pub fn rotate_key(
    data_dir: &Path,
    old: &StorageCrypto,
    new_keys: KeyManager,
) -> Result<RotationReport, StorageError> {
    match old.protection() {
        super::Protection::UnixFile | super::Protection::OpenWrtFile => {}
        other => {
            return Err(StorageError::Platform(format!(
                "当前主密钥由 {} 托管，轮换需在平台侧重新封装凭据；\
                 请参考 README「密钥轮换」一节的说明",
                other.as_str()
            )))
        }
    }

    super::platform::write_pending_key(data_dir, &new_keys.export_master())?;
    let new_id = new_keys.key_id_hex();
    let new = StorageCrypto::new(new_keys);

    let mut report = RotationReport {
        old_key_id: old.key_id_hex(),
        new_key_id: new_id.clone(),
        ..Default::default()
    };

    for id in FileIdentity::ALL {
        if rotate_one(data_dir, id, old, &new, &report.old_key_id)? {
            report.reencrypted.push(id);
        }
    }

    super::platform::promote_pending_key(data_dir)?;

    let mut journal = Journal::load(data_dir);
    journal.key_id = new_id;
    journal.save(data_dir);
    Ok(report)
}

/// 单文件轮换。返回是否真的重写过。
fn rotate_one(
    data_dir: &Path,
    id: FileIdentity,
    old: &StorageCrypto,
    new: &StorageCrypto,
    old_key_id: &str,
) -> Result<bool, StorageError> {
    let path = data_dir.join(id.file_name());
    let Some(blob) = atomic::read_secure(&path, MAX_FILE_BYTES, false)? else {
        return Ok(false);
    };
    if !envelope::is_envelope(&blob) {
        // 明文不可能出现在这里（迁移在启动早期完成）；保守起见报错而非覆盖。
        return Err(StorageError::Corrupt(format!(
            "{} 仍是明文，请先完成迁移再轮换",
            path.display()
        )));
    }

    let plaintext = match old.open(id, &blob) {
        Ok(p) => p,
        Err(StorageError::KeyIdMismatch { .. }) | Err(StorageError::AuthFailed) => {
            // 可能已经轮换过（上次中断），验证后再跳过。
            new.open(id, &blob)?;
            return Ok(false);
        }
        Err(e) => return Err(e),
    };

    let sealed = new.seal(id, &plaintext)?;
    let bak = data_dir.join(format!("{}.rotate-bak-{old_key_id}", id.file_name()));
    atomic::write_atomic(&bak, &blob, 0o600)?;
    atomic::write_atomic(&path, &sealed, 0o600)?;

    // 回读校验
    let check = atomic::read_secure(&path, MAX_FILE_BYTES, false)?
        .ok_or_else(|| StorageError::Io("轮换后文件不可读".into()))?;
    let pt = new.open(id, &check)?;
    if pt.as_slice() != plaintext.as_slice() {
        return Err(StorageError::Corrupt(format!(
            "{} 轮换后内容不一致（旧密文备份：{}）",
            path.display(),
            bak.display()
        )));
    }
    Ok(true)
}

/// 启动时检查是否有"未完成的轮换"，若有则继续做完。
///
/// 返回 `Some(new_key_id)` 表示完成了一次补偿轮换。
pub fn resume_rotation(
    data_dir: &Path,
    current: &StorageCrypto,
) -> Result<Option<String>, StorageError> {
    let Some(pending) = super::platform::read_pending_key(data_dir)? else {
        return Ok(None);
    };
    let new_keys = KeyManager::from_master(pending, current.protection());
    let new_id = new_keys.key_id_hex();
    if new_id == current.key_id_hex() {
        // pending 与当前一致：直接清理
        super::platform::promote_pending_key(data_dir)?;
        return Ok(None);
    }

    eprintln!(
        "[storage] 检测到未完成的密钥轮换（{} → {}），继续完成",
        current.key_id_hex(),
        new_id
    );
    let new = StorageCrypto::new(new_keys);
    for id in FileIdentity::ALL {
        rotate_one(data_dir, id, current, &new, &current.key_id_hex())?;
    }
    super::platform::promote_pending_key(data_dir)?;
    let mut journal = Journal::load(data_dir);
    journal.key_id = new_id.clone();
    journal.save(data_dir);
    Ok(Some(new_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_broken_json() {
        assert!(validate_plaintext(FileIdentity::Accounts, b"{not json").is_err());
        assert!(validate_plaintext(FileIdentity::Accounts, b"[]").is_ok());
        assert!(validate_plaintext(FileIdentity::State, b"{}").is_ok());
        assert!(validate_plaintext(FileIdentity::State, b"").is_err());
    }

    #[test]
    fn scan_detects_envelope_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), b"{\"a\":1}").unwrap();
        assert!(!scan_has_ciphertext(dir.path()));
        let crypto = StorageCrypto::new(KeyManager::from_master(
            zeroize::Zeroizing::new([3u8; 32]),
            super::super::Protection::UnixFile,
        ));
        let sealed = crypto.seal(FileIdentity::Config, b"{\"a\":1}").unwrap();
        std::fs::write(dir.path().join("config.json"), &sealed).unwrap();
        assert!(scan_has_ciphertext(dir.path()));
    }

    #[test]
    fn backup_listing_is_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path()
                .join(backup_name(FileIdentity::Config, "20260101-000000")),
            b"x",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join(backup_name(FileIdentity::Config, "20260202-000000")),
            b"x",
        )
        .unwrap();
        let list = list_backups(dir.path(), FileIdentity::Config);
        assert_eq!(list.len(), 2);
        assert!(list[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("20260202"));
    }
}
