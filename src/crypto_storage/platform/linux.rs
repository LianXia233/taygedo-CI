//! Debian Linux：systemd 凭据优先，其次回退到 0600 密钥文件。
//!
//! ## 推荐部署（Debian 13 / systemd 254+）
//!
//! ```ini
//! # /etc/systemd/system/taygedo.service
//! [Service]
//! LoadCredentialEncrypted=taygedo_storage_key:/etc/taygedo/keys/master.key.enc
//! ```
//!
//! `LoadCredentialEncrypted=` 让 systemd 自己用 TPM2（若机器有 TPM 且
//! 已 `systemd-creds encrypt --with-key=auto`）或主机密钥解封，然后在
//! `$CREDENTIALS_DIRECTORY/taygedo_storage_key` 下提供一份 **tmpfs 中的、
//! 仅本服务可读**的明文副本。是否真正用到 TPM 由 systemd 与该机器的能力
//! 决定，本程序无法也**不会**替 systemd 宣称这一点。
//!
//! ## 凭据内容格式
//!
//! 接受以下任一形式（便于 `systemd-creds` 之外的工具生成）：
//! - 32 字节裸二进制；
//! - 64 个十六进制字符（可带结尾换行）；
//! - 32 字节的 base64 / base64url（可带 `=` 填充）。
//!
//! ## 无凭据设施时
//!
//! `CREDENTIALS_DIRECTORY` 不存在 → 回落到 [`super::super::platform::KEY_FILE_NAME`]，
//! 保护等级标记为 `unix-file-0600`，不做任何美化。

use std::path::PathBuf;

use base64::Engine as _;
use zeroize::Zeroizing;

use super::super::StorageError;

/// systemd 凭据目录（仅在被 systemd 以 `LoadCredential*=` 启动时存在）。
pub fn credentials_dir() -> Option<PathBuf> {
    let v = std::env::var_os("CREDENTIALS_DIRECTORY")?;
    if v.is_empty() {
        return None;
    }
    Some(PathBuf::from(v))
}

/// 凭据文件名。与 unit 里的 `LoadCredentialEncrypted=<name>:<path>` 同名。
pub const CREDENTIAL_NAME: &str = "taygedo_storage_key";

/// 尝试从 systemd 凭据读取主密钥。
///
/// - `Ok(None)`：没有凭据设施，或凭据文件不存在（回退到文件方案）。
/// - `Err`：凭据存在但内容非法（**不**回退，避免掩盖配置错误）。
pub fn credential_key() -> Result<Option<[u8; 32]>, StorageError> {
    let Some(dir) = credentials_dir() else {
        return Ok(None);
    };
    let path = dir.join(CREDENTIAL_NAME);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read(&path).map_err(|e| {
        StorageError::MasterKeyCorrupt(format!("读取 systemd 凭据 {} 失败：{e}", path.display()))
    })?;
    let raw = Zeroizing::new(raw);
    decode_credential(&raw).map(Some)
}

/// 解析凭据内容为 32 字节主密钥。
pub fn decode_credential(raw: &[u8]) -> Result<[u8; 32], StorageError> {
    // 去掉结尾空白（文件形式常带换行）
    let trimmed: &[u8] = {
        let mut end = raw.len();
        while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r' || raw[end - 1] == b' ') {
            end -= 1;
        }
        &raw[..end]
    };

    let mut out = [0u8; 32];
    if trimmed.len() == 32 {
        out.copy_from_slice(trimmed);
        return Ok(out);
    }
    if trimmed.len() == 64 && trimmed.iter().all(|b| b.is_ascii_hexdigit()) {
        for i in 0..32 {
            let hi = hex_val(trimmed[i * 2])?;
            let lo = hex_val(trimmed[i * 2 + 1])?;
            out[i] = (hi << 4) | lo;
        }
        return Ok(out);
    }
    // base64 / base64url
    for eng in [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ] {
        if let Ok(decoded) = eng.decode(trimmed) {
            if decoded.len() == 32 {
                out.copy_from_slice(&decoded);
                return Ok(out);
            }
        }
    }
    Err(StorageError::MasterKeyCorrupt(format!(
        "systemd 凭据 {} 内容非法：期望 32 字节裸数据 / 64 位十六进制 / base64，实际 {} 字节",
        CREDENTIAL_NAME,
        trimmed.len()
    )))
}

fn hex_val(b: u8) -> Result<u8, StorageError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(StorageError::MasterKeyCorrupt("十六进制字符非法".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_raw_hex_and_base64() {
        let key = [0xABu8; 32];
        assert_eq!(decode_credential(&key).unwrap(), key);
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(decode_credential(hex.as_bytes()).unwrap(), key);
        let hex_nl = format!("{hex}\n");
        assert_eq!(decode_credential(hex_nl.as_bytes()).unwrap(), key);
        let b64 = base64::engine::general_purpose::STANDARD.encode(key);
        assert_eq!(decode_credential(b64.as_bytes()).unwrap(), key);
    }

    #[test]
    fn decode_rejects_wrong_length() {
        assert!(decode_credential(b"short").is_err());
        assert!(decode_credential(&[0u8; 31]).is_err());
        // 64 个非十六进制字符也不应被当成 hex
        assert!(decode_credential(&[b'z'; 64]).is_err());
    }
}
