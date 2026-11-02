//! Windows x64：用 DPAPI 保护应用主密钥。
//!
//! ## 安全边界（必须准确理解）
//!
//! - **DPAPI 保护的是"密钥"，不是"配置文件"**。配置文件本身由
//!   XChaCha20-Poly1305 加密，DPAPI 只负责让 `keys/master.key.dpapi`
//!   这个 blob 无法被其它 Windows 账户解开。
//! - 默认使用**当前服务账户**上下文（用户作用域）。密钥密文与账户绑定：
//!   换账户、换机器、重装系统都无法解开。
//! - `TAYGEDO_DPAPI_MACHINE=1` 可切到机器作用域。此时同一台机器上的
//!   **任意**账户只要拿到该文件就能解封——安全边界明显更宽，仅用于
//!   "服务账户会变（如 LocalSystem 与计划任务混用）"的部署。
//! - 不把 DPAPI 密文误当成应用配置密文：两者格式完全不同，也不会互相顶替
//!   （应用密文有 `TGDENV01` Magic 与 AAD 绑定）。
//!
//! ## 为什么不用 CNG / TPM 直接封装
//!
//! TPM 封装在 Windows 上需要 TBS API 与额外权限，且在没有 TPM 的机器上
//! 会失败。DPAPI 是 Windows 官方推荐的应用密钥保护机制（内部可选用 TPM），
//! 兼容性最好。

use std::path::Path;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};

use super::super::atomic;
use super::super::StorageError;
use super::Protection;

/// DPAPI blob 上限（实际约 300 字节）。
const MAX_DPAPI_BLOB: usize = 4096;

fn machine_scope() -> bool {
    std::env::var("TAYGEDO_DPAPI_MACHINE")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn input_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// DPAPI 封装 32 字节主密钥。
fn protect(key: &[u8; 32]) -> Result<Vec<u8>, StorageError> {
    let mut flags = CRYPTPROTECT_UI_FORBIDDEN;
    if machine_scope() {
        flags |= CRYPTPROTECT_LOCAL_MACHINE;
    }
    let input = input_blob(&key[..]);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: 输入/输出指针均指向本栈帧内有效内存；out 由系统分配，
    // 失败时不会写入，成功时用 LocalFree 释放。
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
            &mut out,
        )
    };
    if ok == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(StorageError::Platform(format!(
            "DPAPI CryptProtectData 失败（Win32 错误码 {code}）"
        )));
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Ok(bytes)
}

/// DPAPI 解封。
fn unprotect(blob: &[u8]) -> Result<[u8; 32], StorageError> {
    let mut flags = CRYPTPROTECT_UI_FORBIDDEN;
    if machine_scope() {
        flags |= CRYPTPROTECT_LOCAL_MACHINE;
    }
    let input = input_blob(blob);
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: 同上。
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            flags,
            &mut out,
        )
    };
    if ok == 0 {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return Err(StorageError::MasterKeyCorrupt(format!(
            "DPAPI 解封失败（Win32 错误码 {code}）：密钥不属于当前账户/机器，或被篡改"
        )));
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    if bytes.len() != 32 {
        return Err(StorageError::MasterKeyCorrupt(format!(
            "DPAPI 解出的主密钥长度 {} 应为 32",
            bytes.len()
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    // 尽量清理中间明文
    let mut bytes = bytes;
    zeroize::Zeroize::zeroize(&mut bytes);
    Ok(key)
}

/// 从文件读取并解封主密钥。文件不存在返回 `Ok(None)`。
pub fn unprotect_file(path: &Path) -> Result<Option<([u8; 32], Protection)>, StorageError> {
    let Some(blob) = atomic::read_secure(path, MAX_DPAPI_BLOB, false)? else {
        return Ok(None);
    };
    if blob.len() < 16 {
        return Err(StorageError::MasterKeyCorrupt(format!(
            "{} 不是合法的 DPAPI blob（长度 {}）",
            path.display(),
            blob.len()
        )));
    }
    let key = unprotect(&blob)?;
    let protection = if machine_scope() {
        Protection::DpapiMachine
    } else {
        Protection::DpapiUser
    };
    Ok(Some((key, protection)))
}

/// 封装主密钥并写入文件。
pub fn protect_to_file(path: &Path, key: &[u8; 32]) -> Result<Protection, StorageError> {
    let blob = protect(key)?;
    if blob.len() > MAX_DPAPI_BLOB {
        return Err(StorageError::Platform("DPAPI 输出超长".into()));
    }
    // 已存在则拒绝覆盖：覆盖会让既有密文永久不可解。
    if path.exists() {
        return Err(StorageError::MasterKeyCorrupt(format!(
            "{} 已存在，拒绝覆盖（覆盖会使既有密文永久不可解）",
            path.display()
        )));
    }
    atomic::write_atomic(path, &blob, 0o600)?;
    Ok(if machine_scope() {
        Protection::DpapiMachine
    } else {
        Protection::DpapiUser
    })
}
