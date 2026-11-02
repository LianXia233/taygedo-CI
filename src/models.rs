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
    /// 应用层加密策略："auto" | "always" | "never"。
    ///
    /// - `auto`（默认）：内网/回环来源直通明文，非内网来源强制加密；
    /// - `always`：所有来源都要求加密，未加密请求返回 428；
    /// - `never`：关闭应用层加密（排障/兼容旧客户端）。
    #[serde(default = "default_crypto_policy")]
    pub crypto_policy: String,
    /// 受信来源 CIDR 白名单，逗号分隔。缺省为 RFC1918 + loopback + 链路本地。
    ///
    /// 本服务不提供自有登录界面，业务 API 的"可访问范围"由该白名单界定
    /// （OpenWrt 上另有 LuCI 会话校验）。
    #[serde(default = "default_lan_cidrs")]
    pub lan_cidrs: String,
}

/// 应用层加密的默认策略。
fn default_crypto_policy() -> String {
    "auto".into()
}

/// 默认内网白名单：RFC1918 三段 + loopback + IPv6 ULA / 链路本地。
///
/// 对 `crate::web` 公开：`update_config` 在收到空 `lan_cidrs` 时需回到该默认值，
/// 避免用户清空字段后落入空集合（既不放行任何内网、也无法恢复）。
pub fn default_lan_cidrs() -> String {
    "10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,127.0.0.0/8,169.254.0.0/16,::1/128,fc00::/7,fe80::/10"
        .into()
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
            crypto_policy: default_crypto_policy(),
            lan_cidrs: default_lan_cidrs(),
        }
    }
}

/// 长期身份密钥环（存 data/keyring.json，权限 0600）。
///
/// 仅存放服务端 X25519 长期身份私钥。该私钥**不参与**对称密钥派生，
/// 只用于：① 供客户端固化并校验服务端身份（抗中间人）；
/// ② 握手限速与审计时的稳定标识。真正的会话密钥来自每次握手的临时密钥对，
/// 因此该密钥泄露不会导致历史会话被解密（前向保密仍成立）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyRing {
    /// 格式版本，便于后续轮换。
    pub version: u8,
    /// 服务端长期身份私钥（base64url，32 字节）。
    pub identity_secret: String,
    /// 生成时间（上海时区可读串）。
    pub created_at: String,
    /// 轮换标识：每次生成新的密钥对递增。
    #[serde(default)]
    pub generation: u32,
}

impl KeyRing {
    /// 从密钥环解出 32 字节私钥。
    pub fn secret_bytes(&self) -> Result<[u8; 32], String> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let raw = URL_SAFE_NO_PAD
            .decode(self.identity_secret.trim())
            .map_err(|_| "identity_secret 不是合法 base64url".to_string())?;
        if raw.len() != 32 {
            return Err(format!(
                "identity_secret 长度应为 32 字节，实际 {}",
                raw.len()
            ));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw);
        Ok(out)
    }

    /// 生成新的密钥环。
    pub fn generate() -> Self {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let secret = crate::protocol::x25519_generate_secret();
        Self {
            version: 1,
            identity_secret: URL_SAFE_NO_PAD.encode(secret),
            created_at: crate::time::shanghai_datetime(),
            generation: 1,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
