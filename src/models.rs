//! 数据模型。

use serde::{Deserialize, Serialize};

/// 账号，字段名与上游 `accounts.json` 兼容。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub uid: String,
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "openudid", skip_serializing_if = "Option::is_none")]
    pub openudid: Option<String>,
    #[serde(rename = "vendorid", skip_serializing_if = "Option::is_none")]
    pub vendorid: Option<String>,
    #[serde(rename = "accessToken", skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "laohuToken", skip_serializing_if = "Option::is_none")]
    pub laohu_token: Option<String>,
    #[serde(rename = "laohuUserId", skip_serializing_if = "Option::is_none")]
    pub laohu_user_id: Option<String>,
    #[serde(rename = "tokenUpdatedAt", skip_serializing_if = "Option::is_none")]
    pub token_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(rename = "encryptedPassword", skip_serializing_if = "Option::is_none")]
    pub encrypted_password: Option<EncryptedPassword>,
    #[serde(rename = "roleId", skip_serializing_if = "Option::is_none")]
    pub role_id: Option<String>,
    #[serde(rename = "roleName", skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPassword {
    pub v: u8,
    pub alg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kdf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    pub iv: String,
    pub tag: String,
    pub data: String,
}

/// 全局配置（存 data/config.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub credential_key: String,
    #[serde(default = "default_schedule")]
    pub default_schedule: String,
    /// 账号 id → "HH:MM"
    #[serde(default)]
    pub schedules: std::collections::HashMap<String, String>,
    #[serde(default = "default_true")]
    pub coin_tasks: bool,
    #[serde(default = "default_true")]
    pub cloud_duration: bool,
    #[serde(default = "default_share")]
    pub share_platform: String,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// WebUI 登录用户名（默认 admin）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_username: Option<String>,
    /// WebUI 登录密码哈希（hex(sha256(salt:password))），None 表示未设置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_password_hash: Option<String>,
    /// 密码哈希盐。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_password_salt: Option<String>,
}

fn default_schedule() -> String {
    "06:10".into()
}
fn default_true() -> bool {
    true
}
fn default_share() -> String {
    "qq".into()
}
fn default_retries() -> u32 {
    3
}

impl Default for Config {
    fn default() -> Self {
        Self {
            credential_key: crate::crypto::generate_credential_key(),
            default_schedule: default_schedule(),
            schedules: Default::default(),
            coin_tasks: true,
            cloud_duration: true,
            share_platform: default_share(),
            max_retries: 3,
            web_username: None,
            web_password_hash: None,
            web_password_salt: None,
        }
    }
}

/// 游戏角色（签到用）。
#[derive(Debug, Clone)]
pub struct GameRole {
    pub game_id: String,
    pub role_id: String,
    pub role_name: Option<String>,
    pub game_name: Option<String>,
}

/// 单账号签到结果。
#[derive(Debug, Clone, Serialize)]
pub struct AccountResult {
    pub id: String,
    pub name: String,
    pub status: String, // success | failed | skipped
    pub app_signin: Option<AppSigninResult>,
    pub game_signins: Vec<GameSigninResult>,
    pub coin_tasks: Option<CoinTaskResult>,
    pub cloud_duration: Option<CloudDurationResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppSigninResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_signed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gold_coin: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GameSigninResult {
    pub game_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_name: Option<String>,
    pub role_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward: Option<Reward>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_signed: Option<bool>,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reward {
    pub name: String,
    pub num: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoinTaskResult {
    pub bbs_signin: Option<bool>,
    pub browse_done: i64,
    pub browse_target: i64,
    pub like_done: i64,
    pub like_target: i64,
    pub share_done: i64,
    pub share_target: i64,
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub today_coin: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_coin: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CloudDurationResult {
    pub status: String, // success | skipped | failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gave: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remained: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_reason: Option<String>,
}

/// 整体签到结果。
#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub started_at: String,
    pub finished_at: String,
    pub success_count: usize,
    pub failed_count: usize,
    pub skipped_count: usize,
    pub accounts: Vec<AccountResult>,
    pub summary: String,
}

/// 运行日志条目。
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: String,
    pub message: String,
}
