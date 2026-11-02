//! 加密中间件：请求解密 / 响应加密的唯一入口。
//!
//! 该中间件必须挂在**所有业务路由之外**（`Router::layer`），保证任何
//! `/api/*` 业务 handler 拿到的都是明文 JSON，而离开时一定是密文信封。
//! 业务代码因此不需要感知加密层的存在。
//!
//! ## 强制语义
//!
//! - 带 `X-TGD-Session` 头 → 一律按密文信封处理；解析/认证失败直接返回
//!   加密层错误，**不会**退回按明文处理。
//! - 无信封且策略要求加密 → `428 Precondition Required`。
//! - `crypto_policy=never` 是**显式**的运维关闭项（排障 / 已有 TLS 前置代理），
//!   不属于自动降级：必须在配置或 UCI 中主动写入，且启动横幅会告警。
//! - 错误处理路径本身不会绕过解密：中间件内部任何一步失败都直接返回，
//!   不调用 `next.run()`。
//!
//! ## AAD 绑定
//!
//! `method` 与 `path?query` 取自 HTTP 请求本身（不是客户端声明的），
//! 因此把 `/api/accounts` 的密文搬到 `/api/config` 上必然认证失败。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::service::AppState;

/// 握手路径：不参与加密协商（会话都不存在，无法有信封）。
pub const HANDSHAKE_PATH: &str = "/api/crypto/handshake";

/// 加密会话标识头。
pub const HDR_SID: HeaderName = HeaderName::from_static("x-tgd-session");
/// 响应标记头：告知客户端该响应是否已加密。
pub const HDR_ENC: HeaderName = HeaderName::from_static("x-tgd-enc");
/// GET/HEAD 的信封头。
///
/// HTTP 规范允许 GET 带 body，但 fetch/浏览器/部分代理对 GET body 支持不一致，
/// Node undici 更会直接抛 `Request with GET/HEAD method cannot have body`。
/// 因此无正文语义的方法改为把信封（base64url 的 JSON）放在此请求头里。
/// 头里的信封与 body 里的信封走完全相同的解析与 AAD 校验路径，
/// 不存在"头里可以绕过校验"的旁路。
pub const HDR_ENVELOPE: HeaderName = HeaderName::from_static("x-tgd-envelope");

/// 加密中间件：解密请求体、加密响应体。
///
/// ## 强制语义
///
/// - 路径带 `X-TGD-Session` 头 → 一律按加密信封处理；解析/认证失败直接
///   返回加密层错误，**不会**退回按明文处理。
/// - 未带信封且策略要求加密 → `428 Precondition Required`，响应体明确
///   告知需要握手。
/// - `crypto_policy=never` 是**显式**的运维关闭项（排障/已有 TLS 前置代理），
///   不属于自动降级：它必须在配置或 UCI 中主动写入，且启动横幅会告警。
///
/// ## AAD 绑定
///
/// `method` 与 `path?query` 取自 HTTP 请求本身（不是客户端声明的），
/// 因此把 `/api/accounts` 的密文搬到 `/api/config` 上必然认证失败。
///
/// ## 为什么允许 `clippy::result_large_err`
///
/// 这里的 `Err` 就是「要直接返回给客户端的 HTTP 响应」本身，这是 axum
/// 中间件的既定约定（`Err` 必须实现 `IntoResponse`）。`Response<Body>` 有
/// 128 字节，装箱只能把它变成一次堆分配 + 一次解引用，既不会减少实际成本，
/// 也让每条错误路径多一层包装。lint 的建议（Box 化）在这个签名下没有收益。
#[allow(clippy::result_large_err)]
pub async fn crypto_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    let path = req.uri().path().to_string();
    // 参与 AAD 的路径包含 query，且方法统一大写（与客户端 JS 一致）
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| path.clone());
    let method = req.method().as_str().to_ascii_uppercase();

    // 握手 / 元信息 / 身份公钥 / 静态资源不参与加密协商
    if path == HANDSHAKE_PATH
        || path == "/api/meta"
        || path == "/api/secure/identity"
        || !path.starts_with("/api/")
    {
        return Ok(next.run(req).await);
    }

    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());

    let policy = state.config.read().await.crypto_policy.clone();
    if policy == "never" {
        // 显式关闭：直接透传。启动横幅与 /api/meta 都会暴露该状态。
        let mut resp = next.run(req).await;
        resp.headers_mut()
            .insert(HDR_ENC, HeaderValue::from_static("0"));
        return Ok(resp);
    }

    let require_enc = match policy.as_str() {
        "always" => {
            // OpenWrt 上后端只监听回环，回环调用就是 rpcd/ucode 内部通道：
            // 该通道由 LuCI 会话鉴权，浏览器触达不到，因此不强制信封。
            // 独立部署保持 always 的严格语义（含回环来源）。
            let internal =
                crate::service::is_openwrt() && matches!(peer_ip, Some(ip) if ip.is_loopback());
            !internal
        }
        // auto：回环与内网白名单来源可明文；其余来源必须加密。
        // 回环无条件受信：OpenWrt 的内部通道由 rpcd/ucode 经 loopback 调用，
        // 不能因为用户把 lan_cidrs 改坏就让它 428。
        _ => match peer_ip {
            Some(ip) => !(ip.is_loopback() || state.lan_matches(ip)),
            None => true,
        },
    };

    let has_session = req.headers().contains_key(HDR_SID);

    if !has_session {
        if require_enc {
            return Err(crypto_error_response(
                StatusCode::PRECONDITION_REQUIRED,
                "crypto_required",
                "该来源需要启用应用层加密，请刷新页面完成安全握手",
            ));
        }
        // auto + 受信来源：允许明文（OpenWrt 内部通道 / 内网自用）。这是一条
        // **显式**策略分支，由 crypto_policy 与来源地址共同决定，不是失败后的自动回退。
        let mut resp = next.run(req).await;
        resp.headers_mut()
            .insert(HDR_ENC, HeaderValue::from_static("0"));
        return Ok(resp);
    }

    // ---- 解密请求体 ----
    let (parts, body) = req.into_parts();
    let mut raw = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return Err(crypto_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "请求体过大",
            ))
        }
    };

    // GET/HEAD 等无正文语义的方法：信封在 X-TGD-Envelope 头里。
    // 若 body 非空则以 body 为准——两者同时出现视为请求构造异常，直接拒绝，
    // 避免"头里放一份、body 里放另一份"的歧义被利用。
    if raw.is_empty() {
        if let Some(hv) = parts.headers.get(&HDR_ENVELOPE) {
            let decoded = hv
                .to_str()
                .ok()
                .and_then(|v| super::b64_decode(v.trim()).ok());
            match decoded {
                Some(v) => raw = v.into(),
                None => {
                    return Err(crypto_error_response(
                        StatusCode::BAD_REQUEST,
                        "malformed",
                        "X-TGD-Envelope 头不是合法的 base64url 信封",
                    ))
                }
            }
        }
    } else if parts.headers.contains_key(HDR_ENVELOPE) {
        return Err(crypto_error_response(
            StatusCode::BAD_REQUEST,
            "malformed",
            "同一请求不得同时携带信封正文与 X-TGD-Envelope 头",
        ));
    }

    let (sid, seq, plaintext) = match state.crypto.decrypt_request(&method, &path_and_query, &raw) {
        Ok(v) => v,
        Err(e) => {
            return Err(crypto_error_response(
                e.status(),
                e.public_code(),
                e.public_message(),
            ))
        }
    };

    // 重建请求：body 换成明文，其余（headers/uri）保持不变
    let mut rebuilt =
        axum::extract::Request::from_parts(parts, axum::body::Body::from(plaintext.to_vec()));
    // 信封头已完成使命，不再透传给业务层
    rebuilt.headers_mut().remove(&HDR_ENVELOPE);
    if method != "GET" && method != "HEAD" {
        rebuilt.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    // 把会话号透传给下游（登录成功后据此标记会话已认证）
    rebuilt
        .extensions_mut()
        .insert(crate::web::CryptoSid(sid.clone()));

    // ---- 执行下游业务 ----
    let resp = next.run(rebuilt).await;

    // ---- 加密响应体 ----
    let (mut parts, body) = resp.into_parts();
    let body_bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return Err(crypto_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "body_read_failed",
                "读取响应体失败",
            ))
        }
    };

    match state
        .crypto
        .encrypt_response(&sid, &method, &path_and_query, seq, &body_bytes)
    {
        Ok(ct) => {
            parts.headers.insert(HDR_ENC, HeaderValue::from_static("1"));
            parts.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            parts.headers.remove(header::CONTENT_LENGTH);
            Ok(Response::from_parts(parts, axum::body::Body::from(ct)))
        }
        Err(e) => Err(crypto_error_response(
            e.status(),
            e.public_code(),
            e.public_message(),
        )),
    }
}

/// 请求体上限（34 MiB，覆盖大响应/大请求；信封本身是 base64 文本，
/// 解码后的密文另行限制在 `MAX_CIPHERTEXT`）。
const MAX_BODY_BYTES: usize = 34 * 1024 * 1024;

/// 构造加密层错误响应。
///
/// 响应体是明文 JSON，且**只**含错误码与固定文案——不含路径、密钥、
/// 会话号或任何业务数据，因此不构成"未受保护的敏感信息泄漏"。
fn crypto_error_response(status: StatusCode, code: &str, msg: &str) -> Response {
    let mut resp = (
        status,
        Json(serde_json::json!({ "error": msg, "crypto_error": code })),
    )
        .into_response();
    resp.headers_mut()
        .insert(HDR_ENC, HeaderValue::from_static("0"));
    resp
}
