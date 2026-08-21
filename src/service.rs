//! 应用共享状态与业务编排。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use crate::api::Api;
use crate::login::{self, DeviceIdentity};
use crate::models::{Account, AccountResult, Config, LogEntry, RunResult};
use crate::runner::{self, RunOptions};
use crate::store::Store;

pub struct AppState {
    pub api: Api,
    pub store: Store,
    pub accounts: RwLock<Vec<Account>>,
    pub config: RwLock<Config>,
    pub state: RwLock<HashMap<String, String>>,
    pub logs: std::sync::Mutex<Vec<LogEntry>>,
    pub pending_devices: std::sync::Mutex<HashMap<String, DeviceIdentity>>,
    pub run_lock: Mutex<()>,
    /// 登录会话：token -> 过期 unix 秒。
    pub sessions: std::sync::Mutex<HashMap<String, i64>>,
    /// 免鉴权模式：为 true 时所有 API 无需登录即可访问（内网自用场景）。
    pub no_auth: bool,
}

impl AppState {
    pub fn new(data_dir: std::path::PathBuf) -> Arc<Self> {
        let store = Store::new(data_dir);
        let accounts = store.load_accounts();
        let mut config = store.load_config();

        // 环境变量覆盖（OpenWrt/LuCI 通过 init.d 传入，与 WebUI 全局设置对齐）
        if let Ok(v) = std::env::var("TAYGEDO_DEFAULT_SCHEDULE") {
            if !v.trim().is_empty() {
                config.default_schedule = v.trim().to_string();
            }
        }
        if let Ok(v) = std::env::var("TAYGEDO_COIN_TASKS") {
            config.coin_tasks = parse_bool_env(&v);
        }
        if let Ok(v) = std::env::var("TAYGEDO_CLOUD_DURATION") {
            config.cloud_duration = parse_bool_env(&v);
        }
        if let Ok(v) = std::env::var("TAYGEDO_SHARE_PLATFORM") {
            if !v.trim().is_empty() {
                config.share_platform = v.trim().to_string();
            }
        }

        // 首次启动若没有凭据密钥，立即落盘
        if config.credential_key.is_empty() {
            config.credential_key = crate::crypto::generate_credential_key();
        }

        // 用户名初始化（默认 admin）
        if config.web_username.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            config.web_username = Some("admin".into());
        }

        // WebUI 密码初始化：优先环境变量，否则默认 admin
        let mut initial_password: Option<String> = None;
        if config.web_password_hash.is_none() {
            let env_pwd = std::env::var("TAYGEDO_WEB_PASSWORD")
                .ok()
                .filter(|s| !s.trim().is_empty());
            let password = env_pwd.unwrap_or_else(|| "admin".into());
            let salt = crate::crypto::random_hex(8);
            config.web_password_hash = Some(crate::crypto::hash_password(&password, &salt));
            config.web_password_salt = Some(salt);
            initial_password = Some(password);
        }
        store.save_config(&config);

        // 免鉴权模式：环境变量 TAYGEDO_NO_AUTH=1/true/yes/on 时开启
        let no_auth = std::env::var("TAYGEDO_NO_AUTH")
            .map(|v| parse_bool_env(&v))
            .unwrap_or(false);

        let state = store.load_state();
        let app = Arc::new(Self {
            api: Api::new(),
            store,
            accounts: RwLock::new(accounts),
            config: RwLock::new(config),
            state: RwLock::new(state),
            logs: std::sync::Mutex::new(Vec::new()),
            pending_devices: std::sync::Mutex::new(HashMap::new()),
            run_lock: Mutex::new(()),
            sessions: std::sync::Mutex::new(HashMap::new()),
            no_auth,
        });

        if let Some(pwd) = initial_password {
            app.push_log("warn", format!("已初始化 WebUI 登录账号：admin / {pwd}（登录后请在设置中修改）"));
            println!("[taygedo-rs] WebUI 默认登录账号：admin / {pwd}");
        }
        app
    }

    pub fn push_log(&self, level: &str, message: String) {
        let ts = crate::time::log_ts();
        let mut logs = self.logs.lock().unwrap();
        logs.push(LogEntry {
            ts,
            level: level.to_string(),
            message,
        });
        if logs.len() > 500 {
            let excess = logs.len() - 500;
            logs.drain(0..excess);
        }
    }

    pub fn recent_logs(&self, limit: usize) -> Vec<LogEntry> {
        let logs = self.logs.lock().unwrap();
        let start = logs.len().saturating_sub(limit);
        logs[start..].to_vec()
    }

    /// 校验登录账号密码。
    pub async fn verify_login(&self, username: &str, password: &str) -> bool {
        let config = self.config.read().await;
        let expected_user = config.web_username.as_deref().unwrap_or("admin");
        if username != expected_user {
            return false;
        }
        match (&config.web_password_hash, &config.web_password_salt) {
            (Some(hash), Some(salt)) => crate::crypto::hash_password(password, salt) == *hash,
            _ => false,
        }
    }

    /// 修改登录账号与密码。
    pub async fn set_credentials(&self, username: &str, new_password: &str) {
        let mut config = self.config.write().await;
        if !username.trim().is_empty() {
            config.web_username = Some(username.trim().to_string());
        }
        let salt = crate::crypto::random_hex(8);
        config.web_password_hash = Some(crate::crypto::hash_password(new_password, &salt));
        config.web_password_salt = Some(salt);
        self.store.save_config(&config);
    }

    /// 签发登录 token（有效期 7 天）。
    pub fn issue_token(&self) -> String {
        let token = crate::crypto::random_hex(32);
        let expiry = now_unix() + 7 * 24 * 3600;
        self.sessions.lock().unwrap().insert(token.clone(), expiry);
        token
    }

    /// 校验 token。
    pub fn validate_token(&self, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        let now = now_unix();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, exp| *exp > now);
        sessions.contains_key(token)
    }
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_bool_env(v: &str) -> bool {
    matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// 执行签到（手动触发或调度触发）。`only` 为 None 时运行全部账号。
pub async fn run_signin(state: &AppState, force: bool, only: Option<&[String]>) -> RunResult {
    let _guard = state.run_lock.lock().await;
    let started_at = crate::time::shanghai_datetime();
    let today = crate::time::shanghai_date();

    state.push_log("info", format!("开始执行签到：forceRun={force}"));

    let accounts = state.accounts.read().await.clone();
    let config = state.config.read().await.clone();
    let credential_key = config.credential_key.clone();
    let opts = RunOptions {
        coin_tasks: config.coin_tasks,
        cloud_duration: config.cloud_duration,
        share_platform: config.share_platform.clone(),
    };

    let mut updated_accounts = accounts.clone();
    let mut results: Vec<AccountResult> = Vec::new();
    let state_map = state.state.read().await.clone();

    for account in &accounts {
        if let Some(ids) = only {
            if !ids.contains(&account.id) {
                continue;
            }
        }
        let already = state_map.get(&account.id).map(|d| d == &today).unwrap_or(false);
        if !force && already {
            state.push_log(
                "info",
                format!("账号 {}（{}）：今日已成功签到，跳过", account.name, account.id),
            );
            results.push(AccountResult {
                id: account.id.clone(),
                name: account.name.clone(),
                status: "skipped".into(),
                app_signin: None,
                game_signins: vec![],
                coin_tasks: None,
                cloud_duration: None,
                error: None,
                skipped_reason: Some("今天已成功签到".into()),
            });
            continue;
        }

        state.push_log("info", format!("账号 {}（{}）：开始签到", account.name, account.id));
        let mut cb = |m: String| {
            state.push_log("info", m);
        };
        match runner::run_account(&state.api, account, Some(&credential_key), &opts, &mut cb).await {
            Ok((updated, res)) => {
                for ua in updated_accounts.iter_mut() {
                    if ua.id == account.id {
                        *ua = updated;
                        break;
                    }
                }
                results.push(res);
            }
            Err(e) => {
                state.push_log(
                    "error",
                    format!("账号 {}（{}）：签到失败：{}", account.name, account.id, e.0),
                );
                results.push(AccountResult {
                    id: account.id.clone(),
                    name: account.name.clone(),
                    status: "failed".into(),
                    app_signin: None,
                    game_signins: vec![],
                    coin_tasks: None,
                    cloud_duration: None,
                    error: Some(e.0),
                    skipped_reason: None,
                });
            }
        }
    }

    // 更新已签到状态
    let mut new_state = state_map.clone();
    for r in &results {
        if r.status == "success" {
            new_state.insert(r.id.clone(), today.clone());
        }
    }

    *state.accounts.write().await = updated_accounts.clone();
    state.store.save_accounts(&updated_accounts);
    *state.state.write().await = new_state.clone();
    state.store.save_state(&new_state);

    let success_count = results.iter().filter(|r| r.status == "success").count();
    let failed_count = results.iter().filter(|r| r.status == "failed").count();
    let skipped_count = results.iter().filter(|r| r.status == "skipped").count();
    let finished_at = crate::time::shanghai_datetime();

    let mut result = RunResult {
        started_at,
        finished_at,
        success_count,
        failed_count,
        skipped_count,
        accounts: results,
        summary: String::new(),
    };
    result.summary = runner::summary_text(&result);
    state.push_log("info", result.summary.clone());
    state.push_log(
        "info",
        format!("签到结束：成功 {}，失败 {}，跳过 {}", success_count, failed_count, skipped_count),
    );

    result
}

/// 发送短信验证码。
pub async fn send_code(state: &AppState, phone: &str) -> Result<(), String> {
    let identity = login::generate_device_identity();
    state
        .api
        .send_captcha(phone, &identity.device_id)
        .await
        .map_err(|e| e.0)?;
    state
        .pending_devices
        .lock()
        .unwrap()
        .insert(phone.to_string(), identity);
    Ok(())
}

/// 登录账号（密码或验证码）。
pub async fn login_account(
    state: &AppState,
    phone: &str,
    mode: &str,
    password: Option<&str>,
    captcha: Option<&str>,
    name: Option<&str>,
) -> Result<Account, String> {
    let identity = if mode == "password" {
        login::generate_device_identity()
    } else {
        state
            .pending_devices
            .lock()
            .unwrap()
            .remove(phone)
            .ok_or_else(|| "请先发送验证码".to_string())?
    };

    let (token, user_id) = if mode == "password" {
        let pwd = password.ok_or_else(|| "缺少密码".to_string())?;
        state
            .api
            .login_with_password(phone, pwd, &identity.device_id)
            .await
            .map_err(|e| e.0)?
    } else {
        let cap = captcha.ok_or_else(|| "缺少验证码".to_string())?;
        state
            .api
            .login_with_captcha(phone, cap, &identity.device_id)
            .await
            .map_err(|e| e.0)?
    };

    let (access_token, refresh_token, uid) = state
        .api
        .user_center_login(&token, &user_id, &identity.device_id)
        .await
        .map_err(|e| e.0)?;

    let bind_role = state
        .api
        .get_bind_role(&access_token, &uid, "1256")
        .await
        .unwrap_or(None);

    let mut account = Account {
        id: login::gen_account_id(),
        name: name
            .filter(|n| !n.trim().is_empty())
            .map(|n| n.to_string())
            .unwrap_or_else(|| login::gen_account_name(phone)),
        uid: uid.clone(),
        device_id: identity.device_id,
        openudid: Some(identity.openudid),
        vendorid: Some(identity.vendorid),
        access_token: Some(access_token),
        refresh_token,
        laohu_token: Some(token),
        laohu_user_id: Some(user_id),
        token_updated_at: Some(crate::time::shanghai_datetime()),
        phone: Some(phone.to_string()),
        encrypted_password: None,
        role_id: bind_role.as_ref().map(|(rid, _)| rid.clone()),
        role_name: bind_role.as_ref().and_then(|(_, rn)| rn.clone()),
    };

    if mode == "password" {
        if let Some(pwd) = password {
            let ck = state.config.read().await.credential_key.clone();
            account.encrypted_password = Some(crate::crypto::encrypt_password(pwd, &ck));
        }
    }

    // upsert by phone
    let mut accounts = state.accounts.write().await;
    let existing = accounts.iter().position(|a| a.phone.as_deref() == Some(phone));
    match existing {
        Some(idx) => {
            let old = &accounts[idx];
            account.id = old.id.clone();
            if name.is_none() {
                account.name = old.name.clone();
            }
            accounts[idx] = account.clone();
        }
        None => {
            accounts.push(account.clone());
        }
    }
    state.store.save_accounts(&accounts);

    Ok(account)
}

/// 删除账号。
pub async fn delete_account(state: &AppState, id: &str) -> bool {
    let mut accounts = state.accounts.write().await;
    let before = accounts.len();
    accounts.retain(|a| a.id != id);
    let changed = accounts.len() != before;
    if changed {
        state.store.save_accounts(&accounts);
    }
    changed
}

/// 设置账号签到时间。
pub async fn set_schedule(state: &AppState, id: &str, time: Option<&str>) -> Result<(), String> {
    if let Some(t) = time {
        if !is_valid_hhmm(t) {
            return Err("时间格式应为 HH:MM".into());
        }
    }
    let mut config = state.config.write().await;
    match time {
        Some(t) => {
            config.schedules.insert(id.to_string(), t.to_string());
        }
        None => {
            config.schedules.remove(id);
        }
    }
    state.store.save_config(&config);
    Ok(())
}

fn is_valid_hhmm(t: &str) -> bool {
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() != 2 {
        return false;
    }
    match (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
        (Ok(h), Ok(m)) if h < 24 && m < 60 && parts[0].len() == 2 && parts[1].len() == 2 => true,
        _ => false,
    }
}
