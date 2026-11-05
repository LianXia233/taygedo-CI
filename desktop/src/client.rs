//! 后端 REST API 客户端（无鉴权：普通 JSON 请求，不带 `Authorization` 头）。
//!
//! 桌面形态的服务端在编译期强制免鉴权、只监听 127.0.0.1、仅放行回环来源，
//! 因此客户端不实现任何登录 / 令牌 / 改密逻辑。若请求返回 401 / 403，按
//! `ApiError::Unauthorized` / `ApiError::Forbidden` 明确报错，由状态栏展示，
//! 绝不尝试登录。
//!
//! 请求体字段与 `taygedo-CI/src/web.rs` 的 DTO 严格对应；响应以 serde 逐字段
//! 解析，不使用 `unwrap` / `expect` 处理可失败逻辑。

use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const DEFAULT_API_BASE: &str = "http://127.0.0.1:8787";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("网络请求失败: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("服务返回 HTTP {0}: {1}")]
    Status(StatusCode, String),
    #[error("请求被拒绝（401）：桌面形态应为免鉴权直连回环，请确认服务以 desktop 形态启动；{0}")]
    Unauthorized(String),
    #[error("禁止访问（403）：仅允许本机回环来源，请确认未经过 LAN / 反向代理；{0}")]
    Forbidden(String),
    #[error("响应解析失败: {0}")]
    Decode(String),
    #[error("服务返回业务错误: {0}")]
    Api(String),
}

fn error_message(value: &Value) -> String {
    value
        .get("error")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| "未知错误".to_owned())
}

fn body_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let text = error_message(&value);
        if text != "未知错误" {
            return text;
        }
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "（空响应）".to_owned()
    } else {
        trimmed.chars().take(300).collect()
    }
}

fn check_ok(value: &Value) -> Result<(), ApiError> {
    if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
        return Err(ApiError::Api(error.to_owned()));
    }
    if value.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        return Err(ApiError::Api(error_message(value)));
    }
    Ok(())
}

/// 路径段百分号编码（账号 id 通常为 UUID，仍避免特殊字符注入路径）。
fn segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `GET /api/accounts` 中的一行账号。
#[derive(Debug, Clone, Deserialize)]
pub struct AccountRow {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub role_name: Option<String>,
    #[serde(default)]
    pub schedule: String,
    #[serde(default)]
    pub signed_today: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountsResponse {
    #[serde(default)]
    pub accounts: Vec<AccountRow>,
    #[serde(default)]
    pub today: String,
}

/// `POST /api/accounts` 请求体（对应 `AccountLoginReq`）。
#[derive(Debug, Clone, Serialize)]
pub struct AddAccountRequest {
    pub phone: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captcha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogRow {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogsResponse {
    #[serde(default)]
    pub logs: Vec<LogRow>,
}

/// `GET /api/config` 响应。
///
/// 只声明桌面端**确实会展示**的字段。`crypto_policy` / `lan_cidrs` /
/// `lan_no_auth` 属于 WebUI 的「传输加密」区，桌面单包没有登录态与浏览器侧
/// 信封，那三项在这里没有渲染点——声明出来只会变成死代码。serde 默认忽略
/// 多余的 JSON 字段，因此不声明不影响解析。
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigResponse {
    #[serde(default)]
    pub default_schedule: String,
    #[serde(default)]
    pub coin_tasks: bool,
    #[serde(default)]
    pub cloud_duration: bool,
    #[serde(default)]
    pub share_platform: String,
}

/// `GET /api/meta` 响应（desktop 形态）。
///
/// 这里保留服务端返回的全部字段作为回环 API 契约的镜像；当前 UI 只消费
/// `edition` / `auth_mode`，其余字段留待状态栏与安全边界展示使用。
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct MetaInfo {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub edition: String,
    #[serde(default)]
    pub webui_enabled: bool,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub auth_mode: String,
    #[serde(default)]
    pub no_auth: bool,
    #[serde(default)]
    pub crypto_policy: String,
    #[serde(default)]
    pub crypto_required: bool,
    #[serde(default)]
    pub loopback: bool,
}

/// 回环 API 客户端。内部持有可克隆的 `reqwest::Client`，便于异步命令共享。
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: Arc<String>,
}

impl ApiClient {
    pub fn new(base: impl Into<String>) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(3))
            .build()?;
        Ok(Self {
            http,
            base: Arc::new(base.into().trim_end_matches('/').to_owned()),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(self.url(path))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.post(self.url(path))
    }

    fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.delete(self.url(path))
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = req.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized(body_message(&body)));
        }
        if status == StatusCode::FORBIDDEN {
            return Err(ApiError::Forbidden(body_message(&body)));
        }
        if !status.is_success() {
            return Err(ApiError::Status(status, body_message(&body)));
        }
        serde_json::from_str(&body).map_err(|error| {
            ApiError::Decode(format!(
                "{error}；原始响应: {}",
                body.chars().take(300).collect::<String>()
            ))
        })
    }

    /// `GET /api/meta`：探活并返回形态信息（状态栏读取 `edition` / `auth_mode`）。
    pub async fn meta(&self) -> Result<MetaInfo, ApiError> {
        self.send(self.get("/api/meta")).await
    }

    /// `GET /api/accounts`
    pub async fn list_accounts(&self) -> Result<AccountsResponse, ApiError> {
        self.send(self.get("/api/accounts")).await
    }

    /// `POST /api/accounts`
    pub async fn add_account(&self, req: &AddAccountRequest) -> Result<(), ApiError> {
        let value: Value = self.send(self.post("/api/accounts").json(req)).await?;
        check_ok(&value)
    }

    /// `DELETE /api/accounts/{id}`：返回 `ok` 布尔值。
    pub async fn delete_account(&self, id: &str) -> Result<bool, ApiError> {
        let path = format!("/api/accounts/{}", segment(id));
        let value: Value = self.send(self.delete(&path)).await?;
        check_ok(&value)?;
        Ok(value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    /// `POST /api/accounts/{id}/signin`：返回 `RunResult` 原始 JSON。
    pub async fn signin(&self, id: &str, force: bool) -> Result<Value, ApiError> {
        let path = format!("/api/accounts/{}/signin", segment(id));
        self.send(self.post(&path).json(&json!({ "force": force })))
            .await
    }

    /// `POST /api/accounts/{id}/schedule`
    pub async fn set_schedule(&self, id: &str, time: Option<&str>) -> Result<(), ApiError> {
        let path = format!("/api/accounts/{}/schedule", segment(id));
        let value: Value = self
            .send(self.post(&path).json(&json!({ "time": time })))
            .await?;
        check_ok(&value)
    }

    /// `POST /api/send-code`
    pub async fn send_code(&self, phone: &str) -> Result<Value, ApiError> {
        self.send(self.post("/api/send-code").json(&json!({ "phone": phone })))
            .await
    }

    /// `GET /api/logs?limit=`
    pub async fn logs(&self, limit: usize) -> Result<LogsResponse, ApiError> {
        self.send(self.get(&format!("/api/logs?limit={}", limit.min(500))))
            .await
    }

    /// `GET /api/config`
    pub async fn config(&self) -> Result<ConfigResponse, ApiError> {
        self.send(self.get("/api/config")).await
    }

    /// `POST /api/config`：按补丁更新（仅发送提供的字段）。
    pub async fn update_config(&self, patch: &Value) -> Result<(), ApiError> {
        let value: Value = self.send(self.post("/api/config").json(patch)).await?;
        check_ok(&value)
    }
}
