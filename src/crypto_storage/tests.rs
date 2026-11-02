//! 存储加密层测试：格式、AEAD、密钥隔离、边界拒绝。
//!
//! 这些测试直接调用真实实现（`StorageCrypto` / `envelope`），不复制算法。

use super::*;

fn crypto(seed: u8) -> StorageCrypto {
    StorageCrypto::new(KeyManager::from_master(
        zeroize::Zeroizing::new([seed; 32]),
        Protection::UnixFile,
    ))
}

#[test]
fn roundtrip_basic() {
    let c = crypto(1);
    let pt = br#"{"credential_key":"abc","default_schedule":"06:10"}"#;
    let blob = c.seal(FileIdentity::Config, pt).unwrap();
    assert!(envelope::is_envelope(&blob));
    assert_eq!(blob.len(), HEADER_LEN + pt.len() + TAG_LEN);
    let out = c.open(FileIdentity::Config, &blob).unwrap();
    assert_eq!(out.as_slice(), pt);
}

#[test]
fn roundtrip_empty_payload() {
    let c = crypto(1);
    let blob = c.seal(FileIdentity::State, b"").unwrap();
    assert_eq!(blob.len(), HEADER_LEN + TAG_LEN);
    assert!(c.open(FileIdentity::State, &blob).unwrap().is_empty());
}

#[test]
fn roundtrip_large_and_unicode() {
    let c = crypto(9);
    // 1 MiB 内容，含中文/emoji/控制字符
    let mut pt = String::from("{\"note\":\"账号 测试 🚀 \\u0000\"}");
    while pt.len() < 1024 * 1024 {
        pt.push_str("塔吉多自动签到-taygedo-0123456789,");
    }
    let blob = c.seal(FileIdentity::Accounts, pt.as_bytes()).unwrap();
    let out = c.open(FileIdentity::Accounts, &blob).unwrap();
    assert_eq!(out.as_slice(), pt.as_bytes());
    assert_eq!(std::str::from_utf8(&out).unwrap(), pt);
}

#[test]
fn salt_and_nonce_are_random_per_seal() {
    let c = crypto(2);
    let a = c.seal(FileIdentity::Config, b"same").unwrap();
    let b = c.seal(FileIdentity::Config, b"same").unwrap();
    assert_ne!(a, b, "同一明文两次加密必须不同（随机 salt + nonce）");

    let ha = envelope::parse(&a).unwrap().header;
    let hb = envelope::parse(&b).unwrap().header;
    assert_ne!(ha.salt, hb.salt);
    assert_ne!(ha.nonce, hb.nonce);
    assert_ne!(ha.salt, [0u8; SALT_LEN]);
}

#[test]
fn tampered_ciphertext_fails_auth() {
    let c = crypto(3);
    let mut blob = c.seal(FileIdentity::Config, b"{\"a\":1}").unwrap();
    let last = blob.len() - 1;
    blob[last] ^= 0x01;
    assert_eq!(
        c.open(FileIdentity::Config, &blob).unwrap_err(),
        StorageError::AuthFailed
    );
}

#[test]
fn tampered_header_fails() {
    let c = crypto(3);
    let mut blob = c.seal(FileIdentity::Config, b"{\"a\":1}").unwrap();
    // 改 Salt：文件密钥变化 → 认证失败
    blob[26] ^= 0xff;
    assert!(matches!(
        c.open(FileIdentity::Config, &blob),
        Err(StorageError::AuthFailed)
    ));
    // 改 Nonce → 认证失败
    let mut blob2 = c.seal(FileIdentity::Config, b"{\"a\":1}").unwrap();
    blob2[58] ^= 0xff;
    assert!(matches!(
        c.open(FileIdentity::Config, &blob2),
        Err(StorageError::AuthFailed)
    ));
}

#[test]
fn wrong_key_is_rejected_with_key_id_mismatch() {
    let a = crypto(4);
    let b = crypto(5);
    let blob = a.seal(FileIdentity::Config, b"secret").unwrap();
    match b.open(FileIdentity::Config, &blob) {
        Err(StorageError::KeyIdMismatch { expected, found }) => {
            assert_eq!(expected, b.key_id_hex());
            assert_eq!(found, a.key_id_hex());
        }
        other => panic!("期望 KeyIdMismatch，实际 {other:?}"),
    }
}

#[test]
fn wrong_file_identity_is_rejected() {
    // 同一个加密器、同一把密钥，只是身份不同 → AAD 不同 → 认证失败。
    // 这是"把 accounts 的密文拷到 config.json 上"的密码学防线。
    let c = crypto(6);
    let blob = c.seal(FileIdentity::Accounts, b"[]").unwrap();
    assert_eq!(
        c.open(FileIdentity::Config, &blob).unwrap_err(),
        StorageError::AuthFailed
    );
    assert!(c.open(FileIdentity::Accounts, &blob).is_ok());
}

#[test]
fn unknown_version_is_rejected() {
    let c = crypto(7);
    let mut blob = c.seal(FileIdentity::Config, b"{}").unwrap();
    blob[8] = 2;
    assert_eq!(
        c.open(FileIdentity::Config, &blob).unwrap_err(),
        StorageError::UnsupportedVersion(2)
    );
}

#[test]
fn unknown_alg_is_rejected() {
    let c = crypto(7);
    let mut blob = c.seal(FileIdentity::Config, b"{}").unwrap();
    blob[9] = 9;
    assert_eq!(
        c.open(FileIdentity::Config, &blob).unwrap_err(),
        StorageError::UnsupportedAlg(9)
    );
}

#[test]
fn bad_magic_is_rejected_not_treated_as_plaintext() {
    let c = crypto(8);
    let mut blob = c.seal(FileIdentity::Config, b"{}").unwrap();
    blob[0] = b'{';
    assert_eq!(
        c.open(FileIdentity::Config, &blob).unwrap_err(),
        StorageError::BadMagic
    );
    assert!(!envelope::is_envelope(&blob));
}

#[test]
fn truncated_and_padded_files_are_rejected() {
    let c = crypto(8);
    let blob = c.seal(FileIdentity::Config, b"{\"x\":1}").unwrap();

    let truncated = &blob[..blob.len() - 1];
    assert!(matches!(
        c.open(FileIdentity::Config, truncated),
        Err(StorageError::Corrupt(_))
    ));

    let mut padded = blob.clone();
    padded.push(0);
    assert!(matches!(
        c.open(FileIdentity::Config, &padded),
        Err(StorageError::Corrupt(_))
    ));

    // 只有头部，没有密文
    assert!(matches!(
        c.open(FileIdentity::Config, &blob[..HEADER_LEN]),
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn zero_ct_len_is_rejected() {
    let c = crypto(8);
    let mut blob = c.seal(FileIdentity::Config, b"{}").unwrap();
    blob[82..86].copy_from_slice(&0u32.to_be_bytes());
    assert!(matches!(
        c.open(FileIdentity::Config, &blob),
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn oversized_file_is_rejected_before_allocation() {
    let c = crypto(8);
    let huge = vec![0u8; MAX_FILE_BYTES + 1];
    assert!(matches!(
        envelope::parse(&huge),
        Err(StorageError::TooLarge(_))
    ));
    assert!(matches!(
        c.open(FileIdentity::Config, &huge),
        Err(StorageError::TooLarge(_))
    ));
    // 明文侧同样有上限
    let big = vec![0u8; MAX_PLAINTEXT_BYTES + 1];
    assert!(matches!(
        c.seal(FileIdentity::Config, &big),
        Err(StorageError::TooLarge(_))
    ));
}

#[test]
fn plaintext_json_is_not_an_envelope() {
    // 明文与密文格式必须无法互相混淆
    assert!(!envelope::is_envelope(b"{\"a\":1}"));
    assert!(!envelope::is_envelope(b"[]"));
    assert!(!envelope::is_envelope(b""));
    assert!(!envelope::is_envelope(b"TGDENV0"));
}

#[test]
fn key_ids_differ_between_keys() {
    assert_ne!(crypto(1).key_id_hex(), crypto(2).key_id_hex());
    assert_eq!(crypto(1).key_id_hex(), crypto(1).key_id_hex());
}

#[test]
fn every_identity_produces_distinct_ciphertext() {
    let c = crypto(11);
    let mut seen = std::collections::HashSet::new();
    for id in FileIdentity::ALL {
        let blob = c.seal(id, b"same-plaintext").unwrap();
        // 身份进 AAD，但密文本身也应该不同（文件密钥不同）
        assert!(seen.insert(blob.clone()), "{id:?} 与其它身份产生了相同密文");
        assert!(c.open(id, &blob).is_ok());
    }
}

#[test]
fn error_messages_do_not_leak_secrets() {
    let c = crypto(12);
    let blob = c
        .seal(FileIdentity::Config, b"{\"password\":\"hunter2\"}")
        .unwrap();
    let err = c
        .open(FileIdentity::Accounts, &blob)
        .unwrap_err()
        .to_string();
    assert!(!err.contains("hunter2"));
    assert!(err.contains("认证失败"));
}
