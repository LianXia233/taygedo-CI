//! `Store` 端到端测试：加解密落盘、旧版迁移、密钥丢失/损坏、轮换、并发锁。
//!
//! 这些测试走的是**真实代码路径**（`Store::new` → `KeyManager` → 平台密钥文件
//! → `atomic` 写入 → `migration`），不是单独测一个加密工具函数。

use super::*;
use crate::crypto_storage::{envelope, FileIdentity, Protection};
// 仅在 Unix 用例（中断轮换续做）里用到，不 gate 会在 Windows 上触发 unused_imports。
#[cfg(unix)]
use zeroize::Zeroizing;

fn accounts_json() -> String {
    r#"[
  {
    "id": "acc-1",
    "name": "测试账号",
    "uid": "u-10086",
    "deviceId": "dev-abc",
    "refreshToken": "rt-secret-value",
    "phone": "13800000000"
  }
]"#
    .to_string()
}

fn config_json(cred: &str) -> String {
    format!(
        r#"{{
  "credential_key": "{cred}",
  "default_schedule": "07:30",
  "schedules": {{"acc-1": "08:00"}},
  "coin_tasks": false,
  "cloud_duration": true,
  "share_platform": "qq",
  "max_retries": 3,
  "crypto_policy": "auto",
  "lan_cidrs": "192.168.0.0/16"
}}"#
    )
}

fn keyring_json() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let secret = URL_SAFE_NO_PAD.encode([0x5Au8; 32]);
    format!(
        r#"{{"version":1,"identity_secret":"{secret}","created_at":"2026-01-01 00:00:00","generation":3}}"#
    )
}

/// 构造一个"旧版部署"：数据目录里全是明文 JSON，没有 keys/。
fn seed_legacy(dir: &std::path::Path) {
    std::fs::write(dir.join("accounts.json"), accounts_json()).unwrap();
    std::fs::write(dir.join("config.json"), config_json("legacy-cred-key")).unwrap();
    std::fs::write(dir.join("state.json"), r#"{"acc-1":"2026-01-01"}"#).unwrap();
    std::fs::write(dir.join("keyring.json"), keyring_json()).unwrap();
}

fn read_raw(dir: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap()
}

#[test]
fn save_and_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();

    let accounts: Vec<Account> = serde_json::from_str(&accounts_json()).unwrap();
    store.save_accounts(&accounts).unwrap();
    assert_eq!(store.load_accounts().unwrap().len(), 1);

    let config: Config = serde_json::from_str(&config_json("ck-1")).unwrap();
    store.save_config(&config).unwrap();
    let back = store.load_config().unwrap();
    assert_eq!(back.credential_key, "ck-1");
    assert_eq!(back.schedules.get("acc-1").unwrap(), "08:00");

    let mut st = HashMap::new();
    st.insert("acc-1".to_string(), "2026-02-02".to_string());
    store.save_state(&st).unwrap();
    assert_eq!(
        store.load_state().unwrap().get("acc-1").unwrap(),
        "2026-02-02"
    );
}

// 断言的是 Unix 主密钥文件布局（`keys/master.key`），Windows 走 DPAPI 封装。
#[cfg(unix)]
#[test]
fn on_disk_bytes_are_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    let accounts: Vec<Account> = serde_json::from_str(&accounts_json()).unwrap();
    store.save_accounts(&accounts).unwrap();
    store
        .save_config(&serde_json::from_str::<Config>(&config_json("ck-plain")).unwrap())
        .unwrap();

    for name in ["accounts.json", "config.json"] {
        let raw = read_raw(dir.path(), name);
        assert!(envelope::is_envelope(&raw), "{name} 不是密文");
        let text = String::from_utf8_lossy(&raw);
        assert!(
            !text.contains("rt-secret-value"),
            "{name} 泄漏了 token 明文"
        );
        assert!(!text.contains("ck-plain"), "{name} 泄漏了凭据密钥明文");
        assert!(!text.contains("13800000000"), "{name} 泄漏了手机号");
    }

    // 主密钥文件存在且不是明文密钥
    let key_path = dir.path().join("keys").join("master.key");
    let kraw = std::fs::read(&key_path).unwrap();
    assert_eq!(kraw.len(), crate::crypto_storage::platform::KEY_BLOB_LEN);
    assert_eq!(&kraw[..8], b"TGDKEY01");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "主密钥文件必须 0600");
    }
}

#[test]
fn legacy_plaintext_is_migrated_on_first_start() {
    let dir = tempfile::tempdir().unwrap();
    seed_legacy(dir.path());

    let store = Store::new(dir.path().to_path_buf()).unwrap();

    // 1. 数据内容一致
    assert_eq!(store.load_accounts().unwrap()[0].name, "测试账号");
    assert_eq!(
        store.load_config().unwrap().credential_key,
        "legacy-cred-key"
    );
    assert_eq!(
        store.load_state().unwrap().get("acc-1").unwrap(),
        "2026-01-01"
    );
    // 2. 身份私钥保持原样（不能因为迁移而换身份）
    let kr = store.load_keyring().unwrap().expect("keyring 必须存在");
    assert_eq!(kr.generation, 3);
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    assert_eq!(kr.identity_secret, URL_SAFE_NO_PAD.encode([0x5Au8; 32]));

    // 3. 磁盘上已全部变成密文
    for id in FileIdentity::ALL {
        let raw = read_raw(dir.path(), id.file_name());
        assert!(envelope::is_envelope(&raw), "{} 未迁移", id.file_name());
    }

    // 4. 迁移报告正确
    assert_eq!(store.migration.migrated_count(), 4);

    // 5. 每个文件都有可解开的加密备份
    for id in FileIdentity::ALL {
        let backups = crate::crypto_storage::migration::list_backups(dir.path(), id);
        assert!(!backups.is_empty(), "{} 缺少迁移备份", id.file_name());
        let blob = std::fs::read(&backups[0]).unwrap();
        assert!(envelope::is_envelope(&blob), "备份必须是密文");
    }

    // 6. 备份内容 == 旧版明文
    let backups =
        crate::crypto_storage::migration::list_backups(dir.path(), FileIdentity::Accounts);
    let blob = std::fs::read(&backups[0]).unwrap();
    let pt = store.crypto.open(FileIdentity::Accounts, &blob).unwrap();
    assert_eq!(pt.as_slice(), accounts_json().as_bytes());
}

#[test]
fn migration_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    seed_legacy(dir.path());
    let s1 = Store::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(s1.migration.migrated_count(), 4);
    let before: Vec<Vec<u8>> = FileIdentity::ALL
        .iter()
        .map(|id| read_raw(dir.path(), id.file_name()))
        .collect();
    drop(s1);

    let s2 = Store::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(s2.migration.migrated_count(), 0, "重复启动不得重复迁移");
    let after: Vec<Vec<u8>> = FileIdentity::ALL
        .iter()
        .map(|id| read_raw(dir.path(), id.file_name()))
        .collect();
    assert_eq!(before, after, "已加密文件不应被重写");
}

#[test]
fn invalid_legacy_file_is_left_untouched_and_errors_loudly() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("accounts.json"), b"{not valid json").unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();

    // 迁移阶段跳过（保留原文件）
    let outcomes = &store.migration.outcomes;
    assert!(outcomes.iter().any(|(id, o)| *id == FileIdentity::Accounts
        && *o == crate::crypto_storage::migration::FileOutcome::SkippedInvalid));
    assert_eq!(read_raw(dir.path(), "accounts.json"), b"{not valid json");

    // 读取时必须显式报错，绝不静默返回空列表
    let err = store.load_accounts().unwrap_err();
    assert!(err.contains("不是加密配置"), "实际错误：{err}");
}

#[test]
fn missing_key_with_existing_ciphertext_is_fatal() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .save_config(&serde_json::from_str::<Config>(&config_json("ck")).unwrap())
            .unwrap();
    }
    // 模拟密钥丢失
    std::fs::remove_dir_all(dir.path().join("keys")).unwrap();

    let err = Store::new(dir.path().to_path_buf())
        .err()
        .expect("应当失败");
    assert!(err.contains("主密钥缺失"), "实际错误：{err}");
    // 密文必须原封不动
    assert!(envelope::is_envelope(&read_raw(dir.path(), "config.json")));
}

#[test]
fn corrupted_ciphertext_is_rejected_and_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .save_config(&serde_json::from_str::<Config>(&config_json("ck")).unwrap())
            .unwrap();
    }
    let path = dir.path().join("config.json");
    let mut raw = std::fs::read(&path).unwrap();
    let last = raw.len() - 1;
    raw[last] ^= 0xFF;
    std::fs::write(&path, &raw).unwrap();

    let err = Store::new(dir.path().to_path_buf())
        .err()
        .expect("应当失败");
    assert!(err.contains("认证失败"), "实际错误：{err}");
    assert_eq!(std::fs::read(&path).unwrap(), raw, "失败时不得改写原文件");
}

// 篡改的是 Unix 主密钥文件（`keys/master.key`），Windows 无此文件。
#[cfg(unix)]
#[test]
fn tampered_key_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .save_config(&serde_json::from_str::<Config>(&config_json("ck")).unwrap())
            .unwrap();
    }
    let key_path = dir.path().join("keys").join("master.key");
    let mut k = std::fs::read(&key_path).unwrap();
    k[10] ^= 0xFF; // 破坏密钥体
    std::fs::write(&key_path, &k).unwrap();

    // 密钥被改 → Key ID 变了 → 密文解不开，必须报错而不是重建
    let err = Store::new(dir.path().to_path_buf())
        .err()
        .expect("应当失败");
    assert!(
        err.contains("密钥 ID 不匹配") || err.contains("认证失败"),
        "实际错误：{err}"
    );
}

#[test]
fn loose_key_permissions_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    {
        let _ = Store::new(dir.path().to_path_buf()).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key_path = dir.path().join("keys").join("master.key");
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let err = Store::new(dir.path().to_path_buf())
            .err()
            .expect("应当失败");
        assert!(err.contains("600"), "实际错误：{err}");
    }
}

#[test]
fn second_instance_cannot_open_same_dir() {
    let dir = tempfile::tempdir().unwrap();
    let _first = Store::new(dir.path().to_path_buf()).unwrap();
    // 锁是排他的，第二个实例必须失败（等待 10 秒超时）
    let err = Store::new(dir.path().to_path_buf())
        .err()
        .expect("应当失败");
    assert!(err.contains("已被另一个"), "实际错误：{err}");
}

#[test]
fn keyring_corruption_is_fatal_not_silently_regenerated() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path().to_path_buf()).unwrap();
        store
            .save_keyring_if_absent(
                &serde_json::from_str::<crate::models::KeyRing>(&keyring_json()).unwrap(),
            )
            .unwrap();
    }
    // 用错误的主密钥重新加密 keyring，模拟"文件被替换"
    let other = crate::crypto_storage::KeyManager::from_master(
        zeroize::Zeroizing::new([0x11u8; 32]),
        Protection::UnixFile,
    );
    let other_crypto = crate::crypto_storage::StorageCrypto::new(other);
    let sealed = other_crypto
        .seal(FileIdentity::KeyRing, keyring_json().as_bytes())
        .unwrap();
    std::fs::write(dir.path().join("keyring.json"), sealed).unwrap();

    let err = Store::new(dir.path().to_path_buf())
        .err()
        .expect("应当失败");
    assert!(err.contains("密钥 ID 不匹配"), "实际错误：{err}");
}

// 主密钥轮换目前只在 Unix 实现（Windows 分支显式返回 Platform 错误）。
#[cfg(unix)]
#[test]
fn rotation_preserves_data_and_changes_key_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .save_config(&serde_json::from_str::<Config>(&config_json("ck-rot")).unwrap())
        .unwrap();
    store
        .save_accounts(&serde_json::from_str::<Vec<Account>>(&accounts_json()).unwrap())
        .unwrap();

    let old_id = store.key_id_hex();
    let new_id = store.rotate_storage_key().unwrap();
    assert_ne!(old_id, new_id);
    assert_eq!(store.key_id_hex(), new_id);

    // 数据仍然可读且内容一致
    assert_eq!(store.load_config().unwrap().credential_key, "ck-rot");
    assert_eq!(store.load_accounts().unwrap()[0].id, "acc-1");

    // 旧密文备份保留
    let baks: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("rotate-bak"))
        .collect();
    assert!(!baks.is_empty(), "轮换必须保留旧密文备份");

    // 重开仍可用新密钥
    drop(store);
    let store2 = Store::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(store2.key_id_hex(), new_id);
    assert_eq!(store2.load_config().unwrap().credential_key, "ck-rot");
}

// 同 `rotation_preserves_data_and_changes_key_id`：轮换仅在 Unix 实现。
/// 模拟"轮换做到一半崩溃"：磁盘上 master.key 仍是旧密钥、pending 是新密钥、
/// 且只有部分文件已换成新密钥。
///
/// 关键断言：**一次重启就要恢复**——续做完成后进程内必须重新装载新密钥，
/// 否则本次启动会拿旧密钥去读新密文，直接报错退出（那等于要求用户重启两次）。
#[cfg(unix)]
#[test]
fn interrupted_rotation_is_resumed_in_one_start() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    store
        .save_config(&serde_json::from_str::<Config>(&config_json("ck-resume")).unwrap())
        .unwrap();
    store
        .save_accounts(&serde_json::from_str::<Vec<Account>>(&accounts_json()).unwrap())
        .unwrap();

    let old_id = store.key_id_hex();
    let protection = store.crypto.protection();

    // 造出中断现场：写入 pending 新密钥，并把 accounts.json 换成新密钥的密文
    let new_master = Zeroizing::new([7u8; 32]);
    crate::crypto_storage::platform::write_pending_key(dir.path(), &new_master).unwrap();
    let new_keys = crate::crypto_storage::key_manager::KeyManager::from_master(
        Zeroizing::new(*new_master),
        protection,
    );
    let new_id = new_keys.key_id_hex();
    assert_ne!(old_id, new_id);
    let new_crypto = crate::crypto_storage::StorageCrypto::new(new_keys);

    let acc_path = dir.path().join(FileIdentity::Accounts.file_name());
    let blob = crate::crypto_storage::atomic::read_secure(
        &acc_path,
        crate::crypto_storage::MAX_FILE_BYTES,
        false,
    )
    .unwrap()
    .expect("accounts.json 应存在");
    let pt = store.crypto.open(FileIdentity::Accounts, &blob).unwrap();
    let resealed = new_crypto.seal(FileIdentity::Accounts, &pt).unwrap();
    crate::crypto_storage::atomic::write_atomic(&acc_path, &resealed, 0o600).unwrap();
    drop(store);

    // 一次重启：续做轮换 + 重新装载新密钥 + 数据完好
    let store2 = Store::new(dir.path().to_path_buf()).unwrap();
    assert_eq!(store2.key_id_hex(), new_id, "续做后应使用 pending 的新密钥");
    assert_eq!(
        store2.load_config().unwrap().credential_key,
        "ck-resume",
        "未轮换的文件也要被续做"
    );
    assert_eq!(store2.load_accounts().unwrap()[0].id, "acc-1");
    assert!(
        !dir.path().join("keys/master.key.pending").exists(),
        "pending 应已被提升并清理"
    );
}

#[test]
fn storage_summary_has_no_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().to_path_buf()).unwrap();
    let s = store.storage_summary();
    assert!(s.contains("XChaCha20-Poly1305"));
    assert!(s.contains(&store.key_id_hex()));
    assert!(!s.contains("credential"));
}
