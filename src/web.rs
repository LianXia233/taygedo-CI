//! Web 服务：REST API + 应用层加密。
//!
//! ## 定位
//!
//! 本服务**不提供内置 WebUI**（没有首页、没有自有登录/改密接口，也没有任何
//! 页面资源）。两种部署形态的唯一区别是访问控制来源：
//!
//! - **OpenWrt 集成模式**：仅监听回环，由 LuCI 的 rpcd/ucode 插件在服务端
//!   调用；鉴权由 OpenWrt 的 LuCI/ubus 会话负责（`X-TGD-Luci-Session`）。
//! - **独立部署**（Windows / Linux 服务器 / Docker）：无登录界面，只接受
//!   回环或 `lan_cidrs` 白名单内的来源（部署方须自行限制可达范围）。
//!
//! ## 分层结构
//!
//! ```text
//! 请求
//!  ├─ cors_middleware        来源白名单（仅对已配置来源回显 Origin）
//!  ├─ crypto_middleware      应用层加密：解密请求体 / 加密响应体
//!  │   ├─ /api/crypto/handshake  握手（带速率限制）
//!  │   └─ 其余 /api/*            按策略要求加密
//!  └─ auth_middleware        访问控制：受信来源 +（OpenWrt）LuCI 会话
//! ```
//!
//! ## 加密策略（`crypto_policy`）
//!
//! - `auto`：受信来源（回环/内网白名单）可明文直通；其余来源**强制**加密。
//! - `always`：任何来源都要求加密，未加密请求返回 428。
//! - `never`：完全关闭（排障、或已有 TLS 前置代理时）。
//!
//! 加密中间件对**静态资源**与握手端点一律不干预。OpenWrt 上不存在浏览器侧
//! 消费者（LuCI 经 rpcd 内部通道走回环），因此实际总是明文直通。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::luci_session;
use crate::service::{self, AppState};
use crate::web_security::middleware::crypto_middleware;
use crate::web_security::{self, CryptoError};

/// 握手路径。
const HANDHAKE_PATH: &str = "/api/crypto/handshake";

/// LuCI 会话头（OpenWrt 集成模式）。
///
/// 与加密协议自身的 `X-TGD-Session`（加密会话号）**必须区分**：后者是本服务
/// 与应用层加密握手的会话标识，前者是 OpenWrt 的 LuCI/ubus 会话标识。
const LUCI_SESSION_HEADER: &str = "X-TGD-Luci-Session";

pub fn router(state: Arc<AppState>) -> Router {
    // 公开路由：加密握手 + 服务元信息 + 身份公钥 + 静态资源。
    //
    // 注意：**没有** `/` 首页路由（内置 WebUI 已移除），也没有 `/api/login`
    // 等自有认证接口；对 `/` 的请求由 axum 直接返回 404。
    //
    // 静态资源（含签名清单）必须公开可读——独立部署的外部客户端要先拿到
    // 它们才能建立加密会话；完整性由 Ed25519 签名清单保证，而不是靠鉴权。
    let public = Router::new()
        .route("/api/meta", get(meta_api))
        .route(HANDHAKE_PATH, post(handshake_api))
        .route("/api/secure/identity", get(identity_api))
        .route("/static/manifest.json", get(manifest_api))
        .route("/static/{*path}", get(static_asset));

    // 受保护路由：所有业务 API（不含任何 WebUI 专属接口）
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

#[derive(Deserialize)]
struct HandshakeReq {
    /// 协议版本，必须为 `taygedo/v2`。
    proto: String,
    /// 客户端临时 X25519 公钥（base64url，32 字节）。
    client_pub: String,
    /// 客户端随机数（base64url，32 字节）。
    client_nonce: String,
}

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

/// 签名清单缓存。清单内容在编译期嵌入，运行期不会变化，
/// 只有身份密钥（随进程）会变，因此缓存在进程内即可。
static MANIFEST_CACHE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// 静态资源签名清单。
///
/// 内置 WebUI 已移除，清单**不含首页 `/`**（该路径由 axum 直接 404）。
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

/// 访问控制中间件。
///
/// 两道门，均取自服务端可验证的信息，**不接受请求体声明**：
///
/// 1. **来源**：回环或 `lan_cidrs` 命中的受信地址。非受信来源一律 403，
///    因此即使服务被误监听在 `0.0.0.0`，公网来源也进不来业务接口。
/// 2. **OpenWrt 会话**（仅 OpenWrt 集成模式）：要求 `X-TGD-Luci-Session`，
///    由 rpcd/ucode 内部通道携带。
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
///   不做任何放宽。独立部署（非 OpenWrt）没有 OpenWrt 会话，第 2 道门不生效，
///   等价于"受信内网直通"；这是移除 WebUI 后的既定取舍，README 的"安全边界"
///   一节如实说明。
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

    // 排障降级：显式关闭会话校验时按"仅受信来源放行"处理。
    // OpenWrt 默认开启，正常部署不会走到这一分支。
    if !service::luci_session_required() {
        return next.run(req).await;
    }

    let sid = req
        .headers()
        .get(LUCI_SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let loopback = peer_ip.map(|ip| ip.is_loopback()).unwrap_or(false);
    match session_gate(service::is_openwrt(), loopback, &sid) {
        SessionGate::InternalChannel => next.run(req).await,
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
            next.run(req).await
        }
    }
}

/// 会话门的判定结果（纯函数，便于单测；不含任何 I/O）。
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

/// 会话门判定：把"来源 + 运行形态 + 会话号"压成一个结果码。
///
/// - OpenWrt + 回环 + 形态合法 → `InternalChannel`（不触发 ubus，避免 rpcd 死锁）
/// - OpenWrt + 回环 + 形态非法 → `Malformed`（拒绝）
/// - 非回环来源（OpenWrt 或其它）→ `UbusCheck`，保持原有 ubus 校验强度
/// - 空会话号 → `Missing`
pub fn session_gate(openwrt: bool, loopback: bool, sid: &str) -> SessionGate {
    if sid.is_empty() {
        return SessionGate::Missing;
    }
    if openwrt && loopback {
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
        Ok(CryptoCtx(
            parts.extensions.get::<CryptoSid>().map(|c| c.0.clone()),
        ))
    }
}

/// 服务元信息：供调用方判断加密策略与访问控制方式。
///
/// 注意：此端点免鉴权（调用方需要先据此决定是否建立加密会话），
/// 因此**只返回非敏感的模式标志**，不包含任何凭据、路径或密钥信息。
/// 内置 WebUI 已移除，这里不再有 `webui_enabled` / `must_change_password`
/// 之类与网页登录相关的字段。
async fn meta_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let cfg = state.config.read().await;
    let peer_ip = Some(normalize_ip(addr.ip()));
    let openwrt = service::is_openwrt();
    let session_auth = service::luci_session_required();
    let is_lan = peer_ip
        .map(|ip| is_trusted_source(&state, ip))
        .unwrap_or(false);
    // 与 crypto_middleware 保持一致：OpenWrt 的回环调用是内部通道，始终不收信封。
    let internal = openwrt && peer_ip.map(|ip| ip.is_loopback()).unwrap_or(false);
    let require_enc = match cfg.crypto_policy.as_str() {
        "always" => !internal,
        "never" => false,
        _ => !is_lan,
    };
    // 访问控制方式：OpenWrt 交给 LuCI/ubus 会话；独立部署无登录，仅受信来源。
    let auth_mode = if session_auth {
        "luci_session"
    } else {
        "trusted_source"
    };
    Json(serde_json::json!({
        "ok": true,
        // 运行版本：standalone（独立部署）| openwrt（LuCI 内部 rpcd 通道）
        "edition": if openwrt { "openwrt" } else { "standalone" },
        "auth_mode": auth_mode,
        "crypto_policy": cfg.crypto_policy,
        "crypto_required": require_enc,
        // 协议版本与算法：客户端据此判断"能不能用安全通道"，
        // 不认识就拒绝，而不是退回明文。
        "crypto_proto": crate::web_security::protocol::PROTO,
        "crypto_alg": crate::web_security::protocol::ALG,
        "crypto_identity_kid": state.crypto.identity_kid(),
    }))
}

// ---- DTO ----

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
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    limit: Option<usize>,
}

// ---- handlers ----

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

    const SID: &str = "0123456789abcdef0123456789abcdef";

    /// 方案 A：内部通道（OpenWrt + 回环）只做形态校验即放行，不触发 ubus。
    #[test]
    fn internal_channel_passes_without_ubus() {
        assert_eq!(session_gate(true, true, SID), SessionGate::InternalChannel);
    }

    /// 内部通道上形态非法的会话号必须拒绝（不得因为"来自回环"就跳过校验）。
    #[test]
    fn internal_channel_rejects_malformed_sid() {
        assert_eq!(session_gate(true, true, "short"), SessionGate::Malformed);
        assert_eq!(
            session_gate(true, true, "0123456789abcdef\"}"),
            SessionGate::Malformed
        );
        assert_eq!(session_gate(true, true, ""), SessionGate::Missing);
        let too_long = "a".repeat(129);
        assert_eq!(session_gate(true, true, &too_long), SessionGate::Malformed);
    }

    /// 非回环来源不走内部通道，保持 ubus 二次校验（不放宽外部路径强度）。
    #[test]
    fn non_loopback_source_keeps_ubus_check() {
        assert_eq!(session_gate(true, false, SID), SessionGate::UbusCheck);
        // 独立部署不启用会话门；即便被调用也按需 ubus 校验，不做额外放行
        assert_eq!(session_gate(false, false, SID), SessionGate::UbusCheck);
        // 非 OpenWrt 但来源为回环：同样不进内部通道分支
        assert_eq!(session_gate(false, true, SID), SessionGate::UbusCheck);
        assert_eq!(session_gate(true, false, ""), SessionGate::Missing);
    }

    /// 回环地址归一：`::ffff:127.0.0.1` 必须被识别为回环。
    #[test]
    fn ipv4_mapped_loopback_is_normalized() {
        let v6: std::net::IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(normalize_ip(v6).is_loopback());
        let real_v6: std::net::IpAddr = "::1".parse().unwrap();
        assert!(normalize_ip(real_v6).is_loopback());
    }
}
