//! 加密层错误类型与对外映射。
//!
//! 设计原则：**对外只暴露粗粒度错误码 + 固定文案**。不返回路径、密钥标识、
//! 会话号、内部状态或任何可用于区分"密钥不对"与"密文被改"的细节——
//! 后者属于 padding oracle 类信息泄漏面。
//!
//! 对内（日志/诊断）保留 `Debug`，但日志只写 `public_code()`。

use axum::http::StatusCode;
use serde_json::json;

/// 加密层错误。对外只暴露粗粒度代码，不泄露密码学细节。
#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// 会话不存在 / 已过期 / 服务重启后失效。
    NoSession,
    /// 信封格式非法（长度、编码、字段缺失）。
    Malformed,
    /// 重放或序号过期。
    Replay,
    /// AEAD 认证失败（篡改 / 密钥不符 / AAD 不符）。
    AuthFailed,
    /// 会话请求数超限。
    Exhausted,
    /// 未知协议版本 / 未知 Key ID —— 一律拒绝，不做兼容降级。
    Unsupported,
    /// 握手本身失败（签名、密钥交换）。
    HandshakeFailed,
    /// 该来源握手过于频繁。
    RateLimited,
}

impl CryptoError {
    /// 对外错误码（机读）。
    pub fn public_code(&self) -> &'static str {
        match self {
            CryptoError::NoSession => "no_session",
            CryptoError::Malformed => "malformed",
            CryptoError::Replay => "replay",
            CryptoError::AuthFailed => "auth_failed",
            CryptoError::Exhausted => "session_exhausted",
            CryptoError::Unsupported => "unsupported",
            CryptoError::HandshakeFailed => "handshake_failed",
            CryptoError::RateLimited => "rate_limited",
        }
    }

    /// 对外文案（人读）。不含任何密钥、路径或内部状态。
    pub fn public_message(&self) -> &'static str {
        match self {
            CryptoError::NoSession => "加密会话不存在或已过期，请刷新页面重新建立会话",
            CryptoError::Malformed => "加密报文格式非法",
            CryptoError::Replay => "检测到重放报文，已拒绝；请刷新页面重新建立会话",
            CryptoError::AuthFailed => "密文认证失败（可能被篡改），已拒绝",
            CryptoError::Exhausted => "会话请求数已达上限，请刷新页面重新建立会话",
            CryptoError::Unsupported => "协议版本或密钥标识不受支持，请刷新页面",
            CryptoError::HandshakeFailed => "安全握手失败",
            CryptoError::RateLimited => "握手过于频繁，请稍后重试",
        }
    }

    /// 映射为 HTTP 状态码。
    ///
    /// `428 Precondition Required` 用于"必须先握手"这一类语义（而不是
    /// `401/403`）——它明确表达"缺少前置条件"，且不会触发浏览器的
    /// Basic/Digest 认证弹窗。
    pub fn status(&self) -> StatusCode {
        match self {
            CryptoError::NoSession => StatusCode::PRECONDITION_REQUIRED,
            CryptoError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            CryptoError::Exhausted => StatusCode::TOO_MANY_REQUESTS,
            CryptoError::Unsupported => StatusCode::PRECONDITION_REQUIRED,
            CryptoError::HandshakeFailed
            | CryptoError::Replay
            | CryptoError::AuthFailed
            | CryptoError::Malformed => StatusCode::BAD_REQUEST,
        }
    }
}

/// 握手失败时的响应体（明文 JSON）。
///
/// 这里允许明文，因为此时**还不存在会话密钥**，用密文回应等于让客户端
/// 无法判断失败原因（连错误都解不开）。内容只有固定文案与协议版本，
/// 不含任何敏感信息。
pub fn handshake_error_body(e: &CryptoError) -> serde_json::Value {
    json!({
        "ok": false,
        "error": e.public_message(),
        "crypto_error": e.public_code(),
        "proto": super::protocol::PROTO,
    })
}
