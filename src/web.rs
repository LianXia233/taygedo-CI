//! Web 服务：REST API + 静态 UI + 应用层加密。
//!
//! ## 分层结构
//!
//! ```text
//! 请求
//!  ├─ cors_middleware        来源白名单（仅对已配置来源回显 Origin）
//!  ├─ crypto_middleware      应用层加密：解密请求体 / 加密响应体
//!  │   ├─ /api/crypto/handshake  握手（免鉴权，带速率限制）
//!  │   └─ 其余 /api/*            按策略要求加密
//!  └─ auth_middleware        鉴权：Bearer token（+ 免鉴权模式按内网放行）
//! ```
//!
//! ## 加密策略（`crypto_policy`）
//!
//! - `auto`：内网来源（LAN 白名单）可明文直通；非内网来源**强制**加密。
//!   这是默认值，兼顾"路由器内网 LuCI 直连"与"公网暴露时正文不可读"。
//! - `always`：任何来源都要求加密，未加密请求返回 428。
//! - `never`：完全关闭（排障、或已有 TLS 前置代理时）。
//!
//! 加密中间件对**静态资源**（除 `/` 首页外）与握手端点一律不干预。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::service::{self, AppState};
use crate::session::{self, CryptoError};

/// 加密会话标识头。
const HDR_SID: HeaderName = HeaderName::from_static("x-tgd-session");
/// 加密请求序号头（十进制）。
const HDR_SEQ: HeaderName = HeaderName::from_static("x-tgd-seq");
/// 响应标记头：告知客户端该响应是否已加密。
const HDR_ENC: HeaderName = HeaderName::from_static("x-tgd-enc");
/// 握手路径。
const HANDHAKE_PATH: &str = "/api/crypto/handshake";

pub fn router(state: Arc<AppState>) -> Router {
    // 公开路由：首页 + 登录 + 加密握手 + 服务元信息
    let public = Router::new()
        .route("/", get(index))
        .route("/api/login", post(login_api))
        .route("/api/meta", get(meta_api))
        .route(HANDHAKE_PATH, post(handshake_api));

    // 受保护路由：所有业务 API
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
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    public
        .merge(protected)
        // 顺序敏感：cors 最外层 → crypto 次之 → auth 最内层。
        // axum 的 layer 是"后加的先执行"，故先 merge/auth 再 crypto 再 cors。
        .layer(middleware::from_fn_with_state(state.clone(), crypto_middleware))
        .layer(middleware::from_fn_with_state(state.clone(), cors_middleware))
        .with_state(state)
}

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

    let allowed = origin.as_deref().map(|o| is_origin_allowed(o, &req, &state));

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
                    "Authorization, Content-Type, X-TGD-Session, X-TGD-Seq",
                )
                // 仅对白名单来源声明可变头，便于前端读取加密标记
                .header(
                    header::ACCESS_CONTROL_EXPOSE_HEADERS,
                    "X-TGD-Enc",
                )
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
            HeaderValue::from_static("Authorization, Content-Type, X-TGD-Session, X-TGD-Seq"),
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
    if allowed_origins_from_env().iter().any(|o| o.eq_ignore_ascii_case(origin)) {
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
// 应用层加密中间件
// ---------------------------------------------------------------------------

/// 加密中间件：解密请求体、加密响应体。
///
/// 仅作用于 `/api/*`；握手端点、静态资源、非 API 路径直接透传。
async fn crypto_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, Response> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    // 握手端点自行管理（免鉴权、带独立限速），中间件不介入
    if path == HANDHAKE_PATH {
        return Ok(next.run(req).await);
    }
    // 非 API 路径（首页、静态资源）不加密
    if !path.starts_with("/api/") {
        return Ok(next.run(req).await);
    }

    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());

    let policy = state.config.read().await.crypto_policy.clone();
    let require_enc = match policy.as_str() {
        "always" => true,
        "never" => false,
        // auto：内网来源（LAN 白名单）可明文；其余来源必须加密
        _ => match peer_ip {
            Some(ip) => !state.lan_matches(ip),
            // 无法识别来源时按"必须加密"处理（更安全的一侧）
            None => true,
        },
    };

    let sid = req
        .headers()
        .get(HDR_SID)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let seq = req
        .headers()
        .get(HDR_SEQ)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    // 客户端的握手探测：GET /api/crypto/handshake 之外，用 X-TGD-Probe 头探测
    // 是否要求加密，避免前端为一次探测就建立完整会话。
    let is_probe = req.headers().contains_key("x-tgd-probe");

    let (sid, seq) = match (sid, seq) {
        (Some(sid), Some(seq)) => (sid, seq),
        _ => {
            if require_enc && !is_probe {
                return Err(crypto_error_response(
                    StatusCode::PRECONDITION_REQUIRED,
                    "crypto_required",
                    "该来源需要启用应用层加密，请先刷新页面完成握手",
                ));
            }
            // 明文直通（auto 下的内网来源，或 policy=never）
            let mut resp = next.run(req).await;
            resp.headers_mut()
                .insert(HDR_ENC, HeaderValue::from_static("0"));
            return Ok(resp);
        }
    };

    // ---- 解密请求体 ----
    let (parts, body) = req.into_parts();
    let raw = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return Err(crypto_error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                "请求体过大",
            ))
        }
    };

    let plaintext = if raw.is_empty() {
        // 空体（如 GET）无需解密
        Vec::new()
    } else {
        let env = match std::str::from_utf8(&raw) {
            Ok(s) => s,
            Err(_) => {
                return Err(crypto_error_response(
                    StatusCode::BAD_REQUEST,
                    "malformed",
                    "加密报文应为 base64url 文本",
                ))
            }
        };
        let ct = match session::decode_envelope(env) {
            Ok(c) => c,
            Err(e) => return Err(crypto_error_response(e.status(), e.public_code(), e.public_message())),
        };
        match state.crypto.decrypt_request(&sid, seq, &ct) {
            Ok((pt, _)) => pt,
            Err(e) => return Err(crypto_error_response(e.status(), e.public_code(), e.public_message())),
        }
    };

    // 重建请求：替换 body 为明文，并保留原始 headers/uri
    let mut rebuilt = axum::extract::Request::from_parts(parts, axum::body::Body::from(plaintext));
    // 解密后内容为 JSON，显式声明，避免 handler 的 Json 提取器因缺 Content-Type 失败
    if method != Method::GET && method != Method::HEAD {
        rebuilt.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    // 把当前加密会话号透传给下游 handler（登录成功后据此标记会话已认证）
    rebuilt.extensions_mut().insert(CryptoSid(sid.clone()));

    // ---- 执行下游 ----
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

    // 无需加密的情形：无正文 / 已非 2xx 且为空（如 204）
    if body_bytes.is_empty() {
        parts.headers.insert(HDR_ENC, HeaderValue::from_static("0"));
        return Ok(Response::from_parts(parts, axum::body::Body::empty()));
    }

    match state.crypto.encrypt_response(&sid, seq, &body_bytes) {
        Ok(ct) => {
            let encoded = session::encode_envelope(&ct);
            parts.headers.insert(HDR_ENC, HeaderValue::from_static("1"));
            parts.headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            // 加密后长度变化，移除原 Content-Length 让 axum 重算
            parts.headers.remove(header::CONTENT_LENGTH);
            Ok(Response::from_parts(parts, axum::body::Body::from(encoded)))
        }
        Err(e) => Err(crypto_error_response(
            e.status(),
            e.public_code(),
            e.public_message(),
        )),
    }
}

/// 请求体上限（32 MiB，足以覆盖文件上传；加密后 base64 膨胀约 1.34 倍，
/// 故此处按解密前原始长度计）。
const MAX_BODY_BYTES: usize = 34 * 1024 * 1024;

/// 构造加密层错误响应。响应体为明文 JSON（便于客户端区分加密层与业务层错误）。
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

impl CryptoError {
    /// 映射为 HTTP 状态码。
    pub fn status(&self) -> StatusCode {
        match self {
            CryptoError::NoSession => StatusCode::PRECONDITION_REQUIRED,
            CryptoError::Replay | CryptoError::AuthFailed | CryptoError::Malformed => {
                StatusCode::BAD_REQUEST
            }
            CryptoError::Exhausted => StatusCode::TOO_MANY_REQUESTS,
        }
    }
}

// ---------------------------------------------------------------------------
// 握手
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct HandshakeReq {
    /// 客户端 X25519 公钥（base64url，32 字节）。
    client_pub: String,
    /// 客户端随机数（base64url，16 字节）。
    client_nonce: String,
}

/// 握手：建立加密会话。免鉴权（加密层必须在鉴权之前建立）。
///
/// 带来源级速率限制，避免被用于资源耗尽。
async fn handshake_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<HandshakeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let peer = addr.ip().to_string();
    if !state.crypto.allow_handshake(&peer) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "握手过于频繁，请稍后重试",
                "crypto_error": "rate_limited"
            })),
        ));
    }

    let client_pub = session::decode_fixed::<32>(&req.client_pub).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "client_pub 应为 32 字节 base64url 的 X25519 公钥",
                "crypto_error": "malformed"
            })),
        )
    })?;
    let client_nonce = session::decode_fixed::<16>(&req.client_nonce).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "client_nonce 应为 16 字节 base64url",
                "crypto_error": "malformed"
            })),
        )
    })?;

    let out = state
        .crypto
        .handshake(&client_pub, &client_nonce, &peer)
        .map_err(|e| {
            (
                e.status(),
                Json(serde_json::json!({
                    "error": e.public_message(),
                    "crypto_error": e.public_code()
                })),
            )
        })?;

    Ok(Json(serde_json::json!({
        "ok": true,
        "sid": out.sid,
        "server_pub": session::encode_fixed(&out.server_pub),
        "server_nonce": session::encode_fixed(&out.server_nonce),
        // 服务端长期身份公钥：客户端可固化校验以抵御中间人
        "identity_pub": session::encode_fixed(&state.crypto.server_identity_public()),
        "expires_in": out.expires_in,
        "alg": "X25519+HKDF-SHA256+AES-256-GCM",
        "proto": "taygedo/v1",
    })))
}

// ---------------------------------------------------------------------------
// 鉴权中间件
// ---------------------------------------------------------------------------

/// 鉴权中间件：校验 Bearer token。免鉴权模式按内网来源放行。
async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip());

    if state.no_auth_allows(peer_ip).await {
        // 免鉴权放行：注入合成身份，供下游区分"匿名内网"与"已登录"
        let mut req = req;
        req.extensions_mut().insert(AuthIdentity {
            username: None,
            via_no_auth: true,
            token: None,
        });
        return Ok(next.run(req).await);
    }

    let token = extract_token(&req);
    match state.validate_token(&token) {
        Some(username) => {
            let mut req = req;
            req.extensions_mut().insert(AuthIdentity {
                username: Some(username),
                via_no_auth: false,
                token: Some(token),
            });
            Ok(next.run(req).await)
        }
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

/// 已认证身份，由 `auth_middleware` 注入，供 handler 使用。
///
/// 关键点：敏感操作的身份**只能**从这里取，不接受请求体声明。
#[derive(Clone, Debug)]
pub struct AuthIdentity {
    pub username: Option<String>,
    /// 是否来自免鉴权放行（此时 `username` 为 None）。
    pub via_no_auth: bool,
    pub token: Option<String>,
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

impl AuthIdentity {
    /// 操作主体名（免鉴权时为 "anonymous"）。
    ///
    /// 预留：供需要记录"谁执行的"审计日志复用。
    #[allow(dead_code)]
    pub fn actor(&self) -> &str {
        self.username.as_deref().unwrap_or("anonymous")
    }
}

fn extract_token(req: &axum::extract::Request) -> String {
    // 仅支持 Authorization: Bearer <token>。
    //
    // 已移除 Cookie 承载路径：一是源码中从无 Set-Cookie 写入，该路径为死代码；
    // 二是 Cookie 会随跨站请求自动携带，一旦启用即引入 CSRF 面。
    // 收敛为单一通道可显著缩小攻击面。
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

async fn index() -> Response {
    // no-cache：WebUI 随二进制编译内嵌（include_str!），升级后若浏览器仍用
    // 启发式缓存的旧页面，会出现"新旧界面元素不一致/功能不同步"的假象。
    ([(header::CACHE_CONTROL, "no-cache")], Html(include_str!("ui.html"))).into_response()
}

/// 服务元信息：供前端判断鉴权模式与加密策略。
///
/// 注意：此端点免鉴权（前端在登录前需要据此决定是否跳过登录），
/// 因此**只返回非敏感的模式标志**，不包含任何凭据、路径或密钥信息。
async fn meta_api(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Json<serde_json::Value> {
    let cfg = state.config.read().await;
    let peer_ip = Some(addr.ip());
    let no_auth = state.no_auth_allows(peer_ip).await;
    let is_lan = peer_ip.map(|ip| state.lan_matches(ip)).unwrap_or(false);
    let require_enc = match cfg.crypto_policy.as_str() {
        "always" => true,
        "never" => false,
        _ => !is_lan,
    };
    Json(serde_json::json!({
        "ok": true,
        "no_auth": no_auth,
        "crypto_policy": cfg.crypto_policy,
        "crypto_required": require_enc,
        "crypto_proto": "taygedo/v1",
        "must_change_password": cfg.web_password_must_change,
    }))
}

// ---- DTO ----

#[derive(Deserialize)]
struct LoginReq {
    #[serde(default = "default_username")]
    username: String,
    password: String,
}

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
    /// 兼容字段：旧客户端会回传当前用户名（审计发现的 P2-2）。
    /// 服务端**不采用**该值作为操作主体，仅为兼容序列化而保留。
    #[serde(default)]
    #[allow(dead_code)]
    username: Option<String>,
}

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
    /// 内网是否免鉴权放行。
    lan_no_auth: Option<bool>,
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default)]
    limit: Option<usize>,
}

// ---- handlers ----

async fn login_api(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 当前请求若走密文通道，取出会话号；登录成功后据此标记会话已认证。
    let req_sid = req.extensions().get::<CryptoSid>().map(|c| c.0.clone());

    let body = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|_| err("请求体读取失败"))?;
    let parsed: LoginReq = serde_json::from_slice(&body).map_err(|_| err("请求体不是合法 JSON"))?;

    // ① 失败锁定检查（此前该逻辑已实现但从未被调用，是审计发现的 P0 缺陷）
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

    // ③ 成功：清除失败计数，并签名标记当前加密会话为已认证
    state.reset_login_fails(&parsed.username);

    // ④ 历史 v1 口令哈希就地升级为 scrypt
    if state.upgrade_password_hash_if_legacy(&parsed.password).await {
        state.push_log("info", "已将该口令哈希从 v1(sha256) 升级为 v2(scrypt)".into());
    }

    let token = state.issue_token(&parsed.username);

    // 标记当前加密会话已通过身份认证。用于区分「仅完成密钥协商」与
    // 「已证明持有口令」两种状态，供后续需要更高保证的接口使用。
    if let Some(sid) = req_sid {
        state.crypto.mark_authenticated(&sid);
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "token": token,
        "must_change_password": state.must_change_password().await,
    })))
}

/// 登出：吊销当前会话。
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

async fn change_password(
    State(state): State<Arc<AppState>>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 手动提取 JSON，以便同时读取 AuthIdentity 扩展
    let ident = req.extensions().get::<AuthIdentity>().cloned();

    // 免鉴权模式（OpenWrt 专用）下不提供改密接口，避免公开改密面
    let no_auth = state.no_auth_allows(None).await && state.no_auth;
    if no_auth && ident.as_ref().map(|i| i.via_no_auth).unwrap_or(false) {
        return Err(err("免鉴权模式（OpenWrt 专用）下无需修改密码"));
    }

    let body = axum::body::to_bytes(req.into_body(), 64 * 1024)
        .await
        .map_err(|_| err("请求体读取失败"))?;
    let parsed: PasswordReq = serde_json::from_slice(&body).map_err(|_| err("请求体不是合法 JSON"))?;

    if parsed.new_password.len() < 8 {
        return Err(err("新密码至少 8 位"));
    }
    if parsed.new_password == parsed.old_password {
        return Err(err("新密码不能与原密码相同"));
    }

    // 操作主体：**从服务端会话取**，而非请求体的 username 字段。
    // 免鉴权放行时无登录主体，此时以配置中的当前用户名作为操作目标，
    // 但必须校验旧口令，语义上与登录等价。
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

    // 改账号名或改口令都要重建凭据；set_credentials 内部会吊销全部会话
    // （含加密会话）并清除"必须改口令"标记，因此改密后旧 token 与旧握手
    // 密钥立即失效。
    if let Err(e) = state.set_credentials(&target_user, &parsed.new_password).await {
        return Err(err(&e));
    }

    // 改密后当前会话已被吊销，通知前端重新登录
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
    Ok(Json(serde_json::json!({ "ok": true, "message": "验证码已发送" })))
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
        "lan_no_auth": config.lan_no_auth,
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
                return Err(err("lan_cidrs 含非法网段，应形如 192.168.0.0/16,10.0.0.0/8"));
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
        if let Some(v) = req.lan_no_auth {
            config.lan_no_auth = v;
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
