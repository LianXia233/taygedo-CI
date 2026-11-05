//! Web 服务：业务 REST API（按构建形态挂载不同的入口与鉴权）。
//!
//! ## 两种形态在编译期分离
//!
//! 同一个 crate 通过 Cargo feature 产出两个**互斥**的二进制，运行期不再做
//! 任何形态探测：
//!
//! - **`webui`**（Windows / Debian / Docker / 独立服务器）：自带首页、
//!   登录/登出/改密接口与静态资源，业务 API 需 `Authorization: Bearer <token>`；
//!   另有应用层加密信封（X25519 + Ed25519 + XChaCha20-Poly1305）。
//! - **`openwrt`**：**只有业务 API 与元信息**。没有首页、没有登录/改密、
//!   没有静态资源、没有握手端点——这些路由不是"存在但返回 404/410"，而是
//!   根本不在路由表里。唯一入口是 LuCI，数据由 rpcd/ucode 在服务端经回环
//!   调用，鉴权交给 LuCI/ubus 会话（`X-TGD-Luci-Session`）。
//!
//! ## 分层结构
//!
//! ```text
//! webui                              openwrt
//!  请求                               请求
//!   ├─ cors_middleware                 ├─ cors_middleware
//!   ├─ crypto_middleware               └─ auth_middleware
//!   │   ├─ /api/crypto/handshake           ├─ 来源白名单
//!   │   └─ 其余 /api/* 按策略加解密         └─ LuCI/ubus 会话
//!   └─ auth_middleware（Bearer token）
//! ```
//!
//! ## 加密策略（`crypto_policy`，仅 `webui`）
//!
//! - `auto`：受信来源（回环/内网白名单）可明文直通；其余来源**强制**加密。
//! - `always`：任何来源都要求加密，未加密请求返回 428。
//! - `never`：完全关闭（排障、或已有 TLS 前置代理时）。
//!
//! 加密中间件对**静态资源**与握手端点一律不干预。登录请求同样走信封：
//! 口令绝不能因为"反正是登录接口"而走明文。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
#[cfg(feature = "webui")]
use axum::response::Html;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

#[cfg(feature = "openwrt")]
use crate::luci_session;
use crate::service::{self, AppState};
#[cfg(feature = "webui")]
use crate::web_security::middleware::crypto_middleware;
#[cfg(feature = "webui")]
use crate::web_security::{self, CryptoError};

/// 握手路径（仅 `webui` 形态注册）。
#[cfg(feature = "webui")]
const HANDHAKE_PATH: &str = "/api/crypto/handshake";

/// LuCI 会话头（仅 `openwrt` 形态）。
///
/// 与加密协议自身的 `X-TGD-Session`（加密会话号）**必须区分**：后者是本服务
/// 与应用层加密握手的会话标识，前者是 OpenWrt 的 LuCI/ubus 会话标识。
#[cfg(feature = "openwrt")]
const LUCI_SESSION_HEADER: &str = "X-TGD-Luci-Session";

/// `webui` 形态路由表：首页 + 登录 + 加密握手 + 元信息 + 静态资源。
#[cfg(feature = "webui")]
pub fn router(state: Arc<AppState>) -> Router {
    // 公开路由。静态资源（含签名清单）必须公开可读——客户端要先拿到它们
    // 才能建立加密会话；完整性由 Ed25519 签名清单保证，而不是靠鉴权。
    let public = Router::new()
        .route("/", get(index))
        .route("/api/login", post(login_api))
        .route("/api/meta", get(meta_api))
        .route(HANDHAKE_PATH, post(handshake_api))
        .route("/api/secure/identity", get(identity_api))
        .route("/static/manifest.json", get(manifest_api))
        .route("/static/{*path}", get(static_asset))
        // 浏览器会自动请求 /favicon.ico：给出空响应，避免控制台出现
        // 一条与本服务无关的 404（真正的图标由页面内联的 SVG data URI 提供）。
        .route("/favicon.ico", get(|| async { StatusCode::NO_CONTENT }));

    // 受保护路由：所有业务 API + WebUI 的改密/登出
    let protected = Router::new()
        .route("/api/accounts", get(list_accounts).post(login))
        .route("/api/accounts/{id}", delete(delete_account))
        .route("/api/accounts/{id}/signin", post(signin))
        .route("/api/accounts/{id}/schedule", post(set_schedule))
        .route("/api/send-code", post(send_code))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/password", post(change_password))
        .route("/api/logout", post(logout_api))
        .route("/api/logs", get(get_logs))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    public
        .merge(protected)
        // 顺序敏感：cors 最外层 → crypto 次之 → auth 最内层。
        // axum 的 layer 是"后加的先执行"，故先 merge/auth 再 crypto 再 cors。
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crypto_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            cors_middleware,
        ))
        .with_state(state)
}

/// `openwrt` 形态路由表：**只有**业务 API 与元信息。
///
/// 首页、登录、登出、改密、静态资源、加密握手全部不在表里——请求它们得到的是
/// axum 的默认 404，而不是一条"本服务承认但不提供"的语义化响应。这是刻意
/// 设计的：OpenWrt 上没有任何浏览器侧消费者，保留占位路由只会扩大可探测面，
/// 也会让人误以为存在一个"被关掉的 WebUI"。
///
/// 唯一入口是 LuCI：浏览器 → LuCI 视图 → ubus `luci.taygedo` → rpcd/ucode
/// → `http://127.0.0.1:<port>`（回环内部通道）。
#[cfg(feature = "openwrt")]
pub fn router(state: Arc<AppState>) -> Router {
    // LuCI 的 ucode 插件会先取 `/api/meta` 探活，因此它必须免鉴权。
    let public = Router::new().route("/api/meta", get(meta_api));

    let protected = Router::new()
        .route("/api/accounts", get(list_accounts).post(login))
        .route("/api/accounts/{id}", delete(delete_account))
        .route("/api/accounts/{id}/signin", post(signin))
        .route("/api/accounts/{id}/schedule", post(set_schedule))
        .route("/api/send-code", post(send_code))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/logs", get(get_logs))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // 不挂 crypto_middleware：内部通道是回环明文，且没有握手端点，
    // 挂了只会让每个请求多走一次策略判定，没有任何消费者。
    public
        .merge(protected)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            cors_middleware,
        ))
        .with_state(state)
}

/// `desktop` 形态路由表：**只有本机回环 REST API**。
///
/// Windows 端已放弃 WebUI，交互层是独立的 GPUI 原生客户端（`desktop/`）。
/// 因此首页、静态资源、签名清单、加密握手、登录/改密/登出端点
/// 全部不编译进来；产物中不存在任何 HTML/JS/加密前端字节。
///
/// 安全边界不是口令，而是「只监听回环 + 只接受回环来源」：
/// 即使误把服务绑到 0.0.0.0，非回环请求也会在鉴权层被 403 拒绝。
#[cfg(feature = "desktop")]
pub fn router(state: Arc<AppState>) -> Router {
    let public = Router::new().route("/api/meta", get(meta_api));
    let protected = Router::new()
        .route("/api/accounts", get(list_accounts).post(login))
        .route("/api/accounts/{id}", delete(delete_account))
        .route("/api/accounts/{id}/signin", post(signin))
        .route("/api/accounts/{id}/schedule", post(set_schedule))
        .route("/api/send-code", post(send_code))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/logs", get(get_logs))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    public
        .merge(protected)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            cors_middleware,
        ))
        .with_state(state)
}

/// 跨源预检允许的请求头。
///
/// 必须与前端实际发送的头一致：`X-TGD-Envelope` 是 GET/HEAD 的信封载体，
/// 漏掉它会让 LuCI（同主机不同端口）在预检阶段就被浏览器拦下；
/// `X-TGD-Luci-Session` 是 OpenWrt 集成模式下 LuCI 会话的载体。
const ALLOWED_REQUEST_HEADERS: &str = "Authorization, Content-Type, X-TGD-Session, \
     X-TGD-Luci-Session, X-TGD-Seq, X-TGD-Envelope";

/// CORS 中间件：来源白名单。
///
/// 仅当请求 `Origin` 命中白名单时才回显该 Origin 并声明凭证支持；
/// 不命中则不下发任何 CORS 头（浏览器侧自然拒绝跨源读取）。
/// 同源请求（无 `Origin` 头或 Origin 与 Host 一致）始终放行。
///
/// 白名单来源：`TAYGEDO_ALLOWED_ORIGINS` 环境变量（逗号分隔），
/// 以及同主机的 LuCI 端口（由请求 Host 推导）。
async fn cors_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let allowed = origin
        .as_deref()
        .map(|o| is_origin_allowed(o, &req, &state));

    if req.method() == Method::OPTIONS {
        let mut builder = Response::builder().status(StatusCode::NO_CONTENT);
        if let (Some(o), Some(true)) = (origin.as_deref(), allowed) {
            builder = builder
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, o)
                .header(header::VARY, "Origin")
                .header(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    "GET, POST, DELETE, OPTIONS",
                )
                .header(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    ALLOWED_REQUEST_HEADERS,
                )
                // 仅对白名单来源声明可变头，便于前端读取加密标记
                .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "X-TGD-Enc")
                .header(header::ACCESS_CONTROL_MAX_AGE, "600");
        }
        return Ok(builder.body(axum::body::Body::empty()).unwrap());
    }

    let mut response = next.run(req).await;
    if let (Some(o), Some(true)) = (origin.as_deref(), allowed) {
        let h = response.headers_mut();
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(o).unwrap_or(HeaderValue::from_static("null")),
        );
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
        );
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static(ALLOWED_REQUEST_HEADERS),
        );
        h.insert(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("X-TGD-Enc"),
        );
    }
    Ok(response)
}

/// 判定 Origin 是否被允许。
///
/// 规则（从严到宽）：
/// 1. 命中 `TAYGEDO_ALLOWED_ORIGINS` 显式白名单；
/// 2. 与请求 `Host` 同主机（同源，或同主机的其它端口 —— 覆盖 LuCI 场景）。
fn is_origin_allowed(origin: &str, req: &axum::extract::Request, state: &Arc<AppState>) -> bool {
    if origin == "null" {
        // file:// 等不透明来源一律拒绝
        return false;
    }
    // 显式白名单优先（额外来源由环境变量提供，避免硬编码任何主机名）
    if allowed_origins_from_env()
        .iter()
        .any(|o| o.eq_ignore_ascii_case(origin))
    {
        return true;
    }
    let origin_host = extract_host(origin);
    let req_host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(extract_host);
    let same_host = match (origin_host, req_host) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(&b),
        _ => false,
    };
    // UCI 配置的动态白名单：允许配置中登记的额外主机（如反代域名）。
    // 此处不再读取其他状态，保持判定为纯函数语义。
    let _ = state;
    same_host
}

/// 读取 `TAYGEDO_ALLOWED_ORIGINS` 并解析为列表（每次读取，允许运行期变更）。
fn allowed_origins_from_env() -> Vec<String> {
    std::env::var("TAYGEDO_ALLOWED_ORIGINS")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// 从 `http://host:port` 或 `host:port` 中提取主机部分。
fn extract_host(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let without_scheme = match s.find("://") {
        Some(i) => &s[i + 3..],
        None => s,
    };
    let host_port = without_scheme.split('/').next()?;
    // IPv6 字面量 [::1]:8080
    if let Some(rest) = host_port.strip_prefix('[') {
        return rest.split(']').next().map(|h| h.to_string());
    }
    Some(host_port.split(':').next()?.to_string())
}

// ---------------------------------------------------------------------------
// 握手
// ---------------------------------------------------------------------------

#[cfg(feature = "webui")]
#[derive(Deserialize)]
struct HandshakeReq {
    /// 协议版本，必须为 `taygedo/v2`。
    proto: String,
    /// 客户端临时 X25519 公钥（base64url，32 字节）。
    client_pub: String,
    /// 客户端随机数（base64url，32 字节）。
    client_nonce: String,
}

#[cfg(feature = "webui")]
/// 握手：建立加密会话。免鉴权（会话必须在鉴权之前建立）。
///
/// 响应中包含服务端 Ed25519 对完整 Transcript 的签名；客户端**必须**先
/// 用固定过的公钥验签，验不过就中止——这是抵御中间人的关键一步。
async fn handshake_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<HandshakeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let peer = addr.ip().to_string();

    // 协议版本必须完全匹配：不认识就拒绝，不做"按老版本兼容"的降级
    if req.proto != crate::web_security::protocol::PROTO {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(crate::web_security::handshake_error_body(
                &CryptoError::Unsupported,
            )),
        ));
    }

    if !state.crypto.allow_handshake(&peer) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(crate::web_security::handshake_error_body(
                &CryptoError::RateLimited,
            )),
        ));
    }

    let client_pub = web_security::decode_fixed::<32>(&req.client_pub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(crate::web_security::handshake_error_body(
                &CryptoError::Malformed,
            )),
        )
    })?;
    let client_nonce = web_security::decode_fixed::<32>(&req.client_nonce).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(crate::web_security::handshake_error_body(
                &CryptoError::Malformed,
            )),
        )
    })?;

    let out = state
        .crypto
        .handshake(&client_pub, &client_nonce, &peer)
        .map_err(|e| {
            (
                e.status(),
                Json(crate::web_security::handshake_error_body(&e)),
            )
        })?;

    Ok(Json(
        serde_json::to_value(&out.response).unwrap_or_else(|_| serde_json::json!({"ok": false})),
    ))
}

#[cfg(feature = "webui")]
/// 服务端身份公钥与指纹。
///
/// 仅返回**公钥**与指纹，不含任何私钥材料。客户端首次访问（TOFU）用它
/// 展示指纹供操作者带外核对；核对之后指纹被固定在本地，后续握手强制比对。
async fn identity_api(State(state): State<Arc<AppState>>) -> Response {
    let body = serde_json::json!({
        "ok": true,
        "proto": crate::web_security::protocol::PROTO,
        "alg": crate::web_security::protocol::ALG,
        "kid": state.crypto.identity_kid(),
        "identity_pub": web_security::b64(&state.crypto.identity_public()),
        "fingerprint": state.crypto.identity_fingerprint(),
        "pin_hint": "首次使用请通过带外渠道（启动横幅 / SSH）核对该指纹，然后在浏览器中固定它",
    });
    let mut resp = Json(body).into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[cfg(feature = "webui")]
/// 签名清单缓存。清单内容在编译期嵌入，运行期不会变化，
/// 只有身份密钥（随进程）会变，因此缓存在进程内即可。
static MANIFEST_CACHE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// 内置 WebUI 首页（仅 `webui` 形态存在此路由）。
///
/// **下发的字节必须与签名清单中的 `/` 条目同源**，因此这里直接取
/// `manifest::asset("/")`（编译期嵌入的那份），而不是读外部文件：
/// 一旦允许运行期替换 HTML，清单哈希必然对不上，前端完整性门禁（
/// `web_guard.js` 会逐个校验 `files`，包含 `/`）就会把整个界面拦死——
/// 那等于给自己造了一个"改个页面就登不进去"的坑。
///
/// 需要改界面就重编二进制：签名清单即可信字节的唯一来源。
#[cfg(feature = "webui")]
async fn index() -> Response {
    let content = web_security::manifest::asset("/").unwrap_or("");
    (
        [(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))],
        Html(content),
    )
        .into_response()
}

/// 静态资源签名清单（仅 `webui` 形态注册）。
#[cfg(feature = "webui")]
async fn manifest_api(State(state): State<Arc<AppState>>) -> Response {
    let bytes = MANIFEST_CACHE.get_or_init(|| {
        let m = web_security::manifest::build(
            state.crypto.identity(),
            web_security::now_unix().max(0) as u64,
        );
        serde_json::to_vec(&m).unwrap_or_else(|_| b"{}".to_vec())
    });
    let mut resp = Response::new(axum::body::Body::from(bytes.clone()));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[cfg(feature = "webui")]
/// 静态资源（编译期嵌入，与实际签名的字节完全一致）。
async fn static_asset(Path(path): Path<String>) -> Response {
    let full = format!("/static/{path}");
    let Some(content) = web_security::manifest::asset(&full) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mut resp = Response::new(axum::body::Body::from(content));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(web_security::manifest::content_type(&full)),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

// ---------------------------------------------------------------------------
// 鉴权中间件
// ---------------------------------------------------------------------------

/// 鉴权中间件（`webui` 形态）：受信来源 + Bearer token。
///
/// 两道门，均取自服务端可验证的信息，**不接受请求体声明**：
///
/// 1. **来源**：回环或 `lan_cidrs` 命中的受信地址。非受信来源一律 403，
///    因此即使服务被误监听在 `0.0.0.0`，公网来源也进不来业务接口。
/// 2. **身份**：`Authorization: Bearer <token>`（登录接口签发，7 天有效）。
///
/// 操作主体一律从 [`AuthIdentity`] 取，绝不相信请求体里自称的用户名。
#[cfg(feature = "webui")]
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .map(normalize_ip);

    let trusted = peer_ip.map(|ip| is_trusted_source(&state, ip));
    if !trusted.unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "untrusted_source",
            "仅接受本机或受信内网来源的请求",
        );
    }

    let token = extract_token(&req);
    match state.validate_token(&token) {
        Some(username) => {
            let mut req = req;
            req.extensions_mut().insert(AuthIdentity {
                username: Some(username),
                token: Some(token),
            });
            next.run(req).await
        }
        None => api_error(StatusCode::UNAUTHORIZED, "unauthorized", "未授权：请先登录"),
    }
}

/// 鉴权中间件（`openwrt` 形态）：受信来源 + LuCI/ubus 会话。
///
/// 来源判定与 `webui` 形态一致（回环无条件受信，其余必须命中 `lan_cidrs`）；
/// 身份完全交给 OpenWrt，本服务不签发、不校验任何自有凭据。
#[cfg(feature = "openwrt")]
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .map(normalize_ip);

    let trusted = peer_ip.map(|ip| is_trusted_source(&state, ip));
    if !trusted.unwrap_or(false) {
        return api_error(
            StatusCode::FORBIDDEN,
            "untrusted_source",
            "仅接受本机或受信内网来源的请求",
        );
    }

    openwrt_auth(req, next, peer_ip).await
}

/// 鉴权中间件（`desktop` 形态）：**无口令，只认回环来源**。
///
/// GPUI 客户端与服务端同机运行，桌面形态的信任边界是操作系统账户隔离
/// 与回环监听，而不是 Bearer token。这里刻意不放宽到 `lan_cidrs`：
/// 桌面服务不应被同网段其它主机调用。
#[cfg(feature = "desktop")]
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .map(normalize_ip);

    // 回环是硬条件；再复用统一的来源归一化判定，避免另立一套 IP 语义。
    let trusted = peer_ip
        .map(|ip| ip.is_loopback() && is_trusted_source(&state, ip))
        .unwrap_or(false);
    if !trusted {
        return api_error(
            StatusCode::FORBIDDEN,
            "untrusted_source",
            "仅接受本机回环来源的请求",
        );
    }

    let mut req = req;
    req.extensions_mut().insert(AuthIdentity {
        username: None,
        token: None,
    });
    next.run(req).await
}

/// OpenWrt 集成模式鉴权：受信来源 + 有效 LuCI 会话。
///
/// 两道门槛：
/// 1. **受信通道**：来源必须是回环/内网（`lan_cidrs`），公网来源一律拒绝。
/// 2. **OpenWrt 会话**：请求须携带 LuCI 会话号，由 rpcd 判定有效性。
///    本服务不签发、不校验自有凭据，登录态完全由 OpenWrt 维护。
///
/// 会话这一道分两种路径，信任前提不同（详见 `session_gate`）：
///
/// - **内部通道**（OpenWrt + 回环来源）：会话号只做形态校验。真实鉴权发生在
///   rpcd 一侧 —— LuCI 的每次调用都要先通过 rpcd 的 ACL（`acl.d/luci-app-taygedo.json`
///   按方法粒度声明 read/write），未登录会话根本调不到 `luci.taygedo` 的方法，
///   而 rpcd 插件传给本服务的是它自己的 `request.session`（服务端真值）。
///   此处**不能**再嵌套 `ubus call session get`：rpcd 的 ucode 方法在自身 uloop
///   内同步执行，后端此时发起的 ubus 请求等不到同一进程应答，会卡满超时后被
///   判无效（死锁根因）。
/// - **其它来源**（OpenWrt 下的非回环来源）：保留 `ubus call session get` 校验，
///   不做任何放宽。
#[cfg(feature = "openwrt")]
async fn openwrt_auth(
    req: axum::extract::Request,
    next: Next,
    peer_ip: Option<std::net::IpAddr>,
) -> Response {
    // 排障降级：显式关闭会话校验时按"仅受信来源放行"处理。
    // 正常部署不会走到这一分支。
    if !service::luci_session_required() {
        let mut req = req;
        req.extensions_mut().insert(AuthIdentity {
            username: None,
            token: None,
        });
        return next.run(req).await;
    }

    let sid = req
        .headers()
        .get(LUCI_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let loopback = peer_ip.map(|ip| ip.is_loopback()).unwrap_or(false);
    match session_gate(loopback, &sid) {
        SessionGate::InternalChannel => {
            let mut req = req;
            req.extensions_mut().insert(AuthIdentity {
                // 身份主体由 OpenWrt 会话决定；需要审计时以 LuCI 会话号为准。
                username: None,
                token: None,
            });
            next.run(req).await
        }
        SessionGate::Missing => api_error(
            StatusCode::UNAUTHORIZED,
            "luci_session_missing",
            "缺少 LuCI 会话：请从 LuCI 菜单「服务 → 塔吉多签到」进入",
        ),
        SessionGate::Malformed => api_error(
            StatusCode::UNAUTHORIZED,
            "luci_session_invalid",
            "LuCI 会话无效或已过期：请重新登录路由器管理界面后再打开本页",
        ),
        SessionGate::UbusCheck => {
            if !luci_session::validate(&sid).await {
                return api_error(
                    StatusCode::UNAUTHORIZED,
                    "luci_session_invalid",
                    "LuCI 会话无效或已过期：请重新登录路由器管理界面后再打开本页",
                );
            }
            let mut req = req;
            req.extensions_mut().insert(AuthIdentity {
                username: None,
                token: None,
            });
            next.run(req).await
        }
    }
}

/// 会话门的判定结果（纯函数，便于单测；不含任何 I/O）。
#[cfg(feature = "openwrt")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionGate {
    /// 内部通道放行：OpenWrt + 回环来源 + 会话号形态合法。
    /// 真实鉴权由 rpcd ACL 完成，这里只确认"调用确实经由内部通道"。
    InternalChannel,
    /// 需要交给 ubus 二次校验（OpenWrt 下的非回环来源）。
    UbusCheck,
    /// 缺少会话号。
    Missing,
    /// 会话号形态非法（越界长度或含可注入字符）。
    Malformed,
}

/// 会话门判定：把"来源 + 会话号"压成一个结果码。
///
/// 形态已经是编译期常量（本函数只存在于 `openwrt` 构建里），因此不再需要
/// 一个 `openwrt: bool` 参数——那是运行期分流的残留。
///
/// - 回环 + 形态合法 → `InternalChannel`（不触发 ubus，避免 rpcd 死锁）
/// - 回环 + 形态非法 → `Malformed`（拒绝）
/// - 非回环来源 → `UbusCheck`，保持原有 ubus 校验强度
/// - 空会话号 → `Missing`
#[cfg(feature = "openwrt")]
pub fn session_gate(loopback: bool, sid: &str) -> SessionGate {
    if sid.is_empty() {
        return SessionGate::Missing;
    }
    if loopback {
        return if luci_session::is_well_formed(sid) {
            SessionGate::InternalChannel
        } else {
            SessionGate::Malformed
        };
    }
    SessionGate::UbusCheck
}

/// IPv4-mapped IPv6（`::ffff:127.0.0.1`）归一为 IPv4，避免回环判定失效。
fn normalize_ip(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => std::net::IpAddr::V4(v4),
            None => ip,
        },
        other => other,
    }
}

/// 来源是否受信：回环无条件受信，其它地址按 `lan_cidrs` 白名单判定。
fn is_trusted_source(state: &AppState, ip: std::net::IpAddr) -> bool {
    ip.is_loopback() || state.lan_matches(ip)
}

/// 结构化错误响应（调用方可按 `code` 分支处理）。
fn api_error(status: StatusCode, code: &str, msg: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "ok": false, "code": code, "error": msg })),
    )
        .into_response()
}

/// 已认证身份，由 `auth_middleware` 注入，供 handler 使用。
///
/// 关键点：敏感操作的身份**只能**从这里取，不接受请求体声明。
///
/// OpenWrt 形态下身份完全由 LuCI/ubus 持有，本结构只用于让下游代码
/// 不必区分形态；因此该形态下字段不会被读取。
#[cfg_attr(any(feature = "openwrt", feature = "desktop"), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct AuthIdentity {
    pub username: Option<String>,
    pub token: Option<String>,
}

impl AuthIdentity {
    /// 操作主体名（免鉴权/OpenWrt 会话下为 "anonymous"）。
    ///
    /// 预留：供需要记录"谁执行的"审计日志复用。
    #[allow(dead_code)]
    pub fn actor(&self) -> &str {
        self.username.as_deref().unwrap_or("anonymous")
    }
}

#[cfg(feature = "webui")]
/// 从 `Authorization: Bearer <token>` 提取 token。
///
/// 仅支持单一承载通道：Cookie 会随跨站请求自动携带，一旦启用即引入 CSRF 面；
/// 收敛为显式请求头可显著缩小攻击面。
fn extract_token(req: &axum::extract::Request) -> String {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

#[cfg(feature = "webui")]
/// 当前请求所属的加密会话号，由 `crypto_middleware` 注入。
///
/// 仅在密文通道上存在；明文通道（内网 auto/never 直通、免鉴权）下不会注入。
#[derive(Clone, Debug)]
pub struct CryptoSid(pub String);

/// 可选的加密会话提取器。
///
/// 与直接读 `Extension<CryptoSid>` 的区别：**明文通道下不报错**，而是给出
/// `None`。handler 因此无需区分"当前是否走加密通道"就能安全地拿到会话号，
/// 适合"如果有会话就保留它、没有就按明文逻辑处理"这类场景
/// （典型：`/api/config` 改加密策略时不能把发起请求的会话一并销毁）。
///
/// OpenWrt 形态不挂加密中间件，也不存在加密会话，因此恒为 `None`——
/// 业务 handler 无需为此写形态分支。
pub struct CryptoCtx(pub Option<String>);

impl<S> axum::extract::FromRequestParts<S> for CryptoCtx
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        #[cfg(feature = "webui")]
        {
            Ok(CryptoCtx(
                parts.extensions.get::<CryptoSid>().map(|c| c.0.clone()),
            ))
        }
        #[cfg(any(feature = "openwrt", feature = "desktop"))]
        {
            let _ = parts;
            Ok(CryptoCtx(None))
        }
    }
}

/// 服务元信息（`webui` 形态）。
///
/// 此端点免鉴权（客户端需要先据此决定是否建立加密会话），因此**只返回
/// 非敏感的模式标志**，不包含任何凭据、路径或密钥信息。
#[cfg(feature = "webui")]
async fn meta_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let cfg = state.config.read().await;
    let peer_ip = Some(normalize_ip(addr.ip()));
    let is_lan = peer_ip
        .map(|ip| is_trusted_source(&state, ip))
        .unwrap_or(false);
    // 与 crypto_middleware 保持一致：auto 策略下受信来源可明文。
    let require_enc = match cfg.crypto_policy.as_str() {
        "always" => true,
        "never" => false,
        _ => !is_lan,
    };
    Json(serde_json::json!({
        "ok": true,
        "edition": service::EDITION,
        "webui_enabled": true,
        "auth_mode": "token",
        "crypto_policy": cfg.crypto_policy,
        "crypto_required": require_enc,
        // 协议版本与算法：客户端据此判断"能不能用安全通道"，
        // 不认识就拒绝，而不是退回明文。
        "crypto_proto": crate::web_security::protocol::PROTO,
        "crypto_alg": crate::web_security::protocol::ALG,
        "crypto_identity_kid": state.crypto.identity_kid(),
        "must_change_password": cfg.web_password_must_change,
    }))
}

/// 服务元信息（`openwrt` 形态）。
///
/// LuCI 的 ucode 插件用它探活。该形态没有握手端点，因此**不返回**
/// `crypto_proto` / `crypto_alg` / 身份指纹——返回一组没有消费者的协议
/// 参数，只会让人误以为存在浏览器侧加密通道。
#[cfg(feature = "openwrt")]
async fn meta_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let cfg = state.config.read().await;
    let peer_ip = Some(normalize_ip(addr.ip()));
    let no_auth = state.no_auth_allows(peer_ip).await;
    let session_auth = service::luci_session_required();
    Json(serde_json::json!({
        "ok": true,
        "edition": service::EDITION,
        // LuCI 是唯一入口：不存在"网页登录态"，也没有改密接口。
        "webui_enabled": false,
        "entry": "luci",
        "auth_mode": if session_auth {
            "luci_session"
        } else if no_auth {
            "none"
        } else {
            "trusted_source"
        },
        "no_auth": no_auth,
        "crypto_policy": cfg.crypto_policy,
        // 内部通道是回环明文，本形态不挂信封中间件。
        "crypto_required": false,
    }))
}

/// 服务元信息（`desktop` 形态）。
///
/// GPUI 客户端用它探活并确认形态；不返回任何协议参数或身份指纹，
/// 因为本形态没有浏览器侧加密通道，也没有登录态。
#[cfg(feature = "desktop")]
async fn meta_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let cfg = state.config.read().await;
    Json(serde_json::json!({
        "ok": true,
        "edition": service::EDITION,
        "webui_enabled": false,
        "entry": "gpui",
        "auth_mode": "loopback",
        "no_auth": true,
        "crypto_policy": cfg.crypto_policy,
        "crypto_required": false,
        "loopback": normalize_ip(addr.ip()).is_loopback(),
    }))
}

// ---- DTO ----

/// 自有登录请求（仅 `webui` 形态）。
#[cfg(feature = "webui")]
#[derive(Deserialize)]
struct LoginReq {
    #[serde(default = "default_username")]
    username: String,
    password: String,
}

/// 改密请求（仅 `webui` 形态）。
#[cfg(feature = "webui")]
#[derive(Deserialize)]
struct PasswordReq {
    old_password: String,
    new_password: String,
    /// 可选的新账号名。留空表示只改口令。
    ///
    /// 注意：这是**改成的目标值**，不是身份声明。操作主体一律从服务端
    /// 会话（`AuthIdentity`）解析，因此回传该字段无法越权操作他人账号。
    #[serde(default)]
    new_username: Option<String>,
    /// 兼容字段：旧客户端会回传当前用户名，服务端**不采用**该值作为操作主体。
    #[serde(default)]
    #[allow(dead_code)]
    username: Option<String>,
}

#[cfg(feature = "webui")]
fn default_username() -> String {
    "admin".into()
}

#[derive(Deserialize)]
struct AccountLoginReq {
    phone: String,
    mode: String, // password | captcha
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    captcha: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct SendCodeReq {
    phone: String,
}

#[derive(Deserialize)]
struct SigninReq {
    #[serde(default)]
    force: Option<bool>,
}

#[derive(Deserialize)]
struct ScheduleReq {
    time: Option<String>,
}

#[derive(Deserialize)]
struct ConfigReq {
    default_schedule: Option<String>,
    coin_tasks: Option<bool>,
    cloud_duration: Option<bool>,
    share_platform: Option<String>,
    /// 加密策略（受保护接口，仅已认证主体可改）。
    crypto_policy: Option<String>,
    /// 内网网段白名单（逗号分隔 CIDR）。空字符串表示恢复默认 RFC1918 集合。
    lan_cidrs: Option<String>,
    /// 内网是否免鉴权放行（仅 OpenWrt 排障降级路径生效）。
    lan_no_auth: Option<bool>,
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    limit: Option<usize>,
}

// ---- handlers ----

/// WebUI 登录：校验口令并签发 token（仅 `webui` 形态）。
///
/// 该接口在加密中间件的覆盖范围内（`/api/*`），因此口令不会以明文上线：
/// 策略要求加密时无信封直接 428，而不是"登录接口就放过"。
#[cfg(feature = "webui")]
async fn login_api(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 当前请求若走密文通道，取出会话号；登录成功后据此标记该会话已认证。
    let req_sid = req.extensions().get::<CryptoSid>().map(|c| c.0.clone());

    let body = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|_| err("请求体读取失败"))?;
    let parsed: LoginReq = serde_json::from_slice(&body).map_err(|_| err("请求体不是合法 JSON"))?;

    // ① 失败锁定检查
    if let Some(remaining) = state.login_lock_remaining(&parsed.username) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": format!("登录失败次数过多，请在 {remaining} 秒后重试"),
                "retry_after": remaining
            })),
        ));
    }

    // ② 恒定成本的口令校验，避免通过响应时间区分"用户不存在"与"密码错误"
    let ok = state.verify_login(&parsed.username, &parsed.password).await;
    if !ok {
        state.record_login_fail(&parsed.username);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "账号或密码错误" })),
        ));
    }

    // ③ 成功：清除失败计数
    state.reset_login_fails(&parsed.username);

    // ④ 历史 v1 口令哈希就地升级为 scrypt
    if state
        .upgrade_password_hash_if_legacy(&parsed.password)
        .await
    {
        state.push_log(
            "info",
            "已将该口令哈希从 v1(sha256) 升级为 v2(scrypt)".into(),
        );
    }

    let token = state.issue_token(&parsed.username);

    // 标记当前加密会话已通过身份认证：区分"仅完成密钥协商"与
    // "已证明持有口令"两种状态。
    if let Some(sid) = req_sid {
        state.crypto.mark_authenticated(&sid);
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "token": token,
        "must_change_password": state.must_change_password().await,
    })))
}

/// 登出：吊销当前会话（仅 `webui` 形态）。
#[cfg(feature = "webui")]
async fn logout_api(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Json<serde_json::Value> {
    if let Some(ident) = req.extensions().get::<AuthIdentity>() {
        if let Some(t) = &ident.token {
            state.revoke_session(t);
        }
    }
    Json(serde_json::json!({ "ok": true }))
}

/// 修改 WebUI 登录账号与口令（仅 `webui` 形态）。
///
/// 操作主体从服务端会话取；改密成功会吊销全部登录会话与加密会话，
/// 要求客户端用新凭据重新登录并重新握手。
#[cfg(feature = "webui")]
async fn change_password(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let ident = req.extensions().get::<AuthIdentity>().cloned();
    // 当前加密会话号：改密会销毁其余会话，但要留住这一条把成功响应送回去。
    let crypto_sid = req.extensions().get::<CryptoSid>().map(|c| c.0.clone());

    let body = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|_| err("请求体读取失败"))?;
    let parsed: PasswordReq =
        serde_json::from_slice(&body).map_err(|_| err("请求体不是合法 JSON"))?;

    if parsed.new_password.len() < 8 {
        return Err(err("新密码至少 8 位"));
    }
    if parsed.new_password == parsed.old_password {
        return Err(err("新密码不能与原密码相同"));
    }

    // 操作主体：**从服务端会话取**，而非请求体的 username 字段。
    let actor = ident
        .as_ref()
        .and_then(|i| i.username.clone())
        .unwrap_or_else(|| "admin".to_string());

    if !state.verify_login(&actor, &parsed.old_password).await {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "原账号或密码错误" })),
        ));
    }

    // 目标账号名：留空则沿用当前主体。显式取值时才校验格式。
    let target_user = match parsed.new_username.as_deref().map(str::trim) {
        Some(u) if !u.is_empty() => {
            if u.chars().count() < 3 || u.chars().count() > 32 {
                return Err(err("账号名长度需在 3-32 个字符之间"));
            }
            if !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                return Err(err("账号名只能包含字母、数字、下划线、连字符与点号"));
            }
            u.to_string()
        }
        _ => actor.clone(),
    };

    // set_credentials 内部会吊销全部登录会话与加密会话（保留当前这条），
    // 并清除"必须改口令"标记
    if let Err(e) = state
        .set_credentials(&target_user, &parsed.new_password, crypto_sid.as_deref())
        .await
    {
        return Err(err(&e));
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "口令已修改，请使用新口令重新登录",
        "sessions_revoked": true,
        "username": target_user,
    })))
}

async fn list_accounts(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let accounts = state.accounts.read().await.clone();
    let config = state.config.read().await.clone();
    let today = crate::time::shanghai_date();
    let st = state.state.read().await.clone();

    let list: Vec<serde_json::Value> = accounts
        .iter()
        .map(|a| {
            let schedule = config
                .schedules
                .get(&a.id)
                .cloned()
                .unwrap_or_else(|| config.default_schedule.clone());
            let signed_today = st.get(&a.id).map(|d| d == &today).unwrap_or(false);
            serde_json::json!({
                "id": a.id,
                "name": a.name,
                "phone": mask_phone(a.phone.as_deref()),
                "uid": a.uid,
                "role_name": a.role_name,
                "schedule": schedule,
                "signed_today": signed_today,
                "has_password": a.encrypted_password.is_some(),
            })
        })
        .collect();

    Json(serde_json::json!({ "accounts": list, "today": today }))
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AccountLoginReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.phone.trim().is_empty() {
        return Err(err("手机号不能为空"));
    }
    let mode = req.mode.as_str();
    if mode != "password" && mode != "captcha" {
        return Err(err("mode 必须是 password 或 captcha"));
    }
    service::login_account(
        &state,
        req.phone.trim(),
        mode,
        req.password.as_deref(),
        req.captcha.as_deref(),
        req.name.as_deref(),
    )
    .await
    .map_err(|e| err(&e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn send_code(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SendCodeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.phone.trim().is_empty() {
        return Err(err("手机号不能为空"));
    }
    service::send_code(&state, req.phone.trim())
        .await
        .map_err(|e| err(&e))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "message": "验证码已发送" }),
    ))
}

async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let ok = service::delete_account(&state, &id).await;
    Json(serde_json::json!({ "ok": ok }))
}

async fn signin(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SigninReq>,
) -> Json<serde_json::Value> {
    let force = req.force.unwrap_or(true);
    let result = service::run_signin(&state, force, Some(&[id][..])).await;
    Json(serde_json::to_value(&result).unwrap())
}

async fn set_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ScheduleReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 空字符串视为"恢复默认"
    let time = req.time.as_deref().filter(|t| !t.trim().is_empty());
    service::set_schedule(&state, &id, time)
        .await
        .map_err(|e| err(&e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let config = state.config.read().await.clone();
    Json(serde_json::json!({
        "default_schedule": config.default_schedule,
        "coin_tasks": config.coin_tasks,
        "cloud_duration": config.cloud_duration,
        "share_platform": config.share_platform,
        "crypto_policy": config.crypto_policy,
        "lan_cidrs": config.lan_cidrs,
        "lan_no_auth": config.lan_no_auth,
    }))
}

async fn update_config(
    State(state): State<Arc<AppState>>,
    cx: CryptoCtx,
    Json(req): Json<ConfigReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 策略字段先在锁外校验并归一化，避免持锁做解析
    let mut new_policy: Option<String> = None;
    if let Some(p) = req.crypto_policy.as_deref() {
        match service::normalize_crypto_policy(p) {
            Some(normalized) => new_policy = Some(normalized),
            None => return Err(err("crypto_policy 只能是 auto / always / never")),
        }
    }
    let mut new_cidrs: Option<String> = None;
    if let Some(c) = req.lan_cidrs.as_deref() {
        // 空串 = 恢复默认；非空则必须每个网段都能解析，否则整次请求拒绝
        // （避免写入半截配置导致内网免鉴权判定异常）
        if !c.trim().is_empty() {
            let parsed = service::LanMatcher::from_cidrs(c);
            if parsed.is_err() {
                return Err(err(
                    "lan_cidrs 含非法网段，应形如 192.168.0.0/16,10.0.0.0/8",
                ));
            }
        }
        new_cidrs = Some(c.trim().to_string());
    }

    let policy_changed;
    let mut cidrs_to_apply: Option<String> = None;
    {
        let mut config = state.config.write().await;
        if let Some(t) = req.default_schedule {
            if !crate::time::valid_hhmm(&t) {
                return Err(err("default_schedule 格式应为 HH:MM"));
            }
            config.default_schedule = t;
        }
        if let Some(v) = req.coin_tasks {
            config.coin_tasks = v;
        }
        if let Some(v) = req.cloud_duration {
            config.cloud_duration = v;
        }
        if let Some(v) = req.share_platform {
            config.share_platform = v;
        }
        if let Some(v) = req.lan_no_auth {
            config.lan_no_auth = v;
        }
        if let Some(p) = new_policy {
            policy_changed = config.crypto_policy != p;
            config.crypto_policy = p;
        } else {
            policy_changed = false;
        }
        if let Some(c) = new_cidrs {
            config.lan_cidrs = if c.is_empty() {
                crate::models::default_lan_cidrs()
            } else {
                c
            };
            cidrs_to_apply = Some(config.lan_cidrs.clone());
        }
        state.store.save_config(&config).map_err(|e| err(&e))?;
    }

    // 配置已落盘后再热更新运行时匹配器（在锁外做，避免持写锁构造解析器）
    if let Some(c) = cidrs_to_apply {
        if let Err(e) = state.set_lan_cidrs(&c) {
            return Err(err(&format!("内网网段已保存但运行时更新失败：{e}")));
        }
    }

    // 策略由明文改为强制加密（或反向）时，既有加密会话的握手前提已变，
    // 主动作废可避免客户端继续复用旧协商结果。
    //
    // 关键：**保留发起本次请求的会话**（`cx.sid`）。否则本请求的响应会因
    // 会话已被自己销毁而无法加密返回，客户端收到 428 / no_session，
    // 误判为失败，而配置其实已经写盘生效。
    if policy_changed {
        let n = match cx.0.as_deref() {
            Some(sid) => state.crypto.destroy_all_except(sid),
            None => state.crypto.destroy_all(),
        };
        state.push_log(
            "info",
            format!("加密策略已变更，已作废 {n} 个加密会话以强制重新握手"),
        );
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn get_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LogsQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(200).min(500);
    let logs = state.recent_logs(limit);
    Json(serde_json::json!({ "logs": logs }))
}

// ---- helpers ----

fn err(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
    )
}

fn mask_phone(p: Option<&str>) -> Option<String> {
    p.map(|p| {
        if p.len() >= 7 {
            format!("{}****{}", &p[..3], &p[p.len() - 4..])
        } else {
            p.to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回环地址归一：`::ffff:127.0.0.1` 必须被识别为回环。
    ///
    /// 两种形态都依赖这个归一（来源判定要用），因此不放进 openwrt 专属用例。
    #[test]
    fn ipv4_mapped_loopback_is_normalized() {
        let v6: std::net::IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(normalize_ip(v6).is_loopback());
        let real_v6: std::net::IpAddr = "::1".parse().unwrap();
        assert!(normalize_ip(real_v6).is_loopback());
    }
}

#[cfg(all(test, feature = "openwrt"))]
mod openwrt_tests {
    use super::*;

    const SID: &str = "0123456789abcdef0123456789abcdef";

    /// 内部通道（回环）只做形态校验即放行，不触发 ubus。
    #[test]
    fn internal_channel_passes_without_ubus() {
        assert_eq!(session_gate(true, SID), SessionGate::InternalChannel);
    }

    /// 内部通道上形态非法的会话号必须拒绝（不得因为"来自回环"就跳过校验）。
    #[test]
    fn internal_channel_rejects_malformed_sid() {
        assert_eq!(session_gate(true, "short"), SessionGate::Malformed);
        assert_eq!(
            session_gate(true, "0123456789abcdef\"}"),
            SessionGate::Malformed
        );
        assert_eq!(session_gate(true, ""), SessionGate::Missing);
        let too_long = "a".repeat(129);
        assert_eq!(session_gate(true, &too_long), SessionGate::Malformed);
    }

    /// 非回环来源不走内部通道，保持 ubus 二次校验（不放宽外部路径强度）。
    #[test]
    fn non_loopback_source_keeps_ubus_check() {
        assert_eq!(session_gate(false, SID), SessionGate::UbusCheck);
        assert_eq!(session_gate(false, ""), SessionGate::Missing);
    }
}
