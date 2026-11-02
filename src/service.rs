//! 应用共享状态与业务编排。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use crate::api::Api;
use crate::login::{self, DeviceIdentity};
use crate::models::{Account, AccountResult, Config, LogEntry, RunResult};
use crate::runner::{self, RunOptions};
use crate::store::Store;
use crate::web_security::CryptoManager;

pub struct AppState {
    pub api: Api,
    pub store: Store,
    pub accounts: RwLock<Vec<Account>>,
    pub config: RwLock<Config>,
    pub state: RwLock<HashMap<String, String>>,
    pub logs: std::sync::Mutex<Vec<LogEntry>>,
    /// 验证码设备身份：phone -> (identity, 发送时间 unix 秒)。带 TTL，防内存泄漏。
    pub pending_devices: std::sync::Mutex<HashMap<String, (DeviceIdentity, i64)>>,
    pub run_lock: Mutex<()>,
    /// 应用层加密会话管理器（独立部署用的 HTTP 信封通道）。
    pub crypto: CryptoManager,
    /// 受信来源判定器。运行期可经 `/api/config` 热更新，故用 Arc 包裹。
    pub lan: std::sync::RwLock<Arc<LanMatcher>>,
}

impl AppState {
    /// 构造应用状态。
    ///
    /// 返回 `Err` 表示**不可启动**：主密钥缺失/损坏、密文认证失败、
    /// 旧版数据无法迁移等。调用方必须打印原因并退出，不允许带着
    /// "没有密钥"的状态继续提供服务（那会静默丢数据）。
    pub fn new(data_dir: std::path::PathBuf) -> Result<Arc<Self>, String> {
        let store = Store::new(data_dir)?;
        let accounts = store.load_accounts()?;
        let mut config = store.load_config()?;

        // 环境变量覆盖（向后兼容：高级用户可手动设置覆盖 config.json；
        // OpenWrt/LuCI 的 init.d 已不再传入业务配置，业务配置统一由 config.json 管理）
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
        // 应用层加密策略可由环境变量覆盖（OpenWrt UCI 经 init.d 传入）
        if let Ok(v) = std::env::var("TAYGEDO_CRYPTO_POLICY") {
            if let Some(p) = normalize_crypto_policy(&v) {
                config.crypto_policy = p;
            }
        }
        // 内网网段白名单覆盖：空值视为「未设置」，保留 config.json 中的值
        // （把空串写进去会把白名单清空，导致内网免鉴权与 auto 豁免双双失效）
        if let Ok(v) = std::env::var("TAYGEDO_LAN_CIDRS") {
            if !v.trim().is_empty() {
                config.lan_cidrs = v.trim().to_string();
            }
        }

        // 首次启动若没有凭据密钥，立即落盘
        if config.credential_key.is_empty() {
            config.credential_key = crate::crypto::generate_credential_key();
        }

        // 长期身份密钥环：不存在则生成；**损坏或解不开一律报错**。
        //
        // 旧实现在解析失败时"用内存临时密钥继续跑"，这在加密存储之后是危
        // 险的：临时密钥意味着本次运行的身份与历史不一致，客户端指纹校验
        // 会静默失效。现在改为显式失败，运维按 README 处理。
        let keyring = store.load_keyring()?;
        let (identity_secret, keyring_created) = match keyring {
            Some(kr) => {
                let s = kr
                    .secret_bytes()
                    .map_err(|e| format!("keyring.json 中的身份私钥非法：{e}"))?;
                (s, false)
            }
            None => {
                let kr = crate::models::KeyRing::generate();
                let s = kr.secret_bytes().expect("新生成的密钥必合法");
                if store.save_keyring_if_absent(&kr)? {
                    (s, true)
                } else {
                    return Err("keyring.json 在初始化期间被其它进程创建，且无法加载；\
                         请检查数据目录权限后重试"
                        .into());
                }
            }
        };
        let crypto = CryptoManager::new(&identity_secret);

        // 内置 WebUI 与自有口令体系已移除：不再生成、也不再迁移任何登录凭据。
        store.save_config(&config)?;

        let lan = LanMatcher::new(&config.lan_cidrs);

        let run_state = store.load_state()?;
        let app = Arc::new(Self {
            api: Api::new(),
            store,
            accounts: RwLock::new(accounts),
            config: RwLock::new(config),
            state: RwLock::new(run_state),
            logs: std::sync::Mutex::new(Vec::new()),
            pending_devices: std::sync::Mutex::new(HashMap::new()),
            run_lock: Mutex::new(()),
            crypto,
            lan: std::sync::RwLock::new(Arc::new(lan)),
        });

        if keyring_created {
            app.push_log(
                "info",
                "已生成长期身份密钥 data/keyring.json（权限 600）。请纳入备份，\
                 丢失后需重新固化客户端指纹。"
                    .into(),
            );
        }
        if let Some(w) = app.store.keyring_permission_warning() {
            app.push_log("warn", format!("密钥文件权限过宽：{w}"));
        }
        // 存储层自检结果：主密钥保护方式 / Key ID / 本次迁移情况。
        // 只包含非敏感元数据，绝不输出密钥材料。
        app.push_log("info", app.store.storage_summary());
        if !app.store.key_protection().os_managed()
            && crate::crypto_storage::platform::platform_supports_os_keystore()
        {
            app.push_log(
                "warn",
                format!(
                    "主密钥当前由 {} 保护。检测到 systemd 凭据设施（$CREDENTIALS_DIRECTORY）可用，\
                     建议改用 LoadCredentialEncrypted= 由 systemd（可配合 TPM2）托管，见 README「平台密钥保护」",
                    app.store.key_protection().as_str()
                ),
            );
        }
        // 访问控制方式：OpenWrt 上额外校验 LuCI/ubus 会话；其它平台只接受
        // 回环/受信内网来源（本服务已无自有登录界面）。
        let auth_note = if luci_session_required() {
            "LuCI 会话（OpenWrt 内部 ucode/rpcd 通道）"
        } else {
            "受信来源（回环/内网白名单，无内置登录）"
        };
        app.push_log("info", format!("访问控制：{auth_note}"));
        Ok(app)
    }

    /// 按当前（可热更新的）内网网段判定来源 IP 是否属于 LAN。
    pub fn lan_matches(&self, ip: std::net::IpAddr) -> bool {
        self.lan.read().map(|m| m.contains(ip)).unwrap_or(false)
    }

    /// 用新的 CIDR 串替换内网匹配器（配置变更时热更新）。
    pub fn set_lan_cidrs(&self, cidrs: &str) -> Result<(), String> {
        let matcher = LanMatcher::from_cidrs(cidrs)?;
        let mut guard = self.lan.write().map_err(|_| "内网匹配器锁中毒")?;
        *guard = Arc::new(matcher);
        Ok(())
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
}

/// 受信来源匹配器。
///
/// 用于两处判断：
/// 1. 业务 API 是否接受该来源的请求（OpenWrt 内部通道/内网自用）；
/// 2. `crypto_policy = auto` 时该来源是否必须启用应用层加密。
///
/// 回环地址始终视为受信（内部通道不依赖用户配置是否把 127.0.0.1 写进白名单）。
pub struct LanMatcher {
    nets: Vec<(ipnet::IpNet, bool)>,
}

impl LanMatcher {
    /// 从逗号分隔的 CIDR 串构造。非法条目被忽略并告警（不 panic）。
    pub fn new(cidrs: &str) -> Self {
        let mut nets = Vec::new();
        for part in cidrs.split(',') {
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            match t.parse::<ipnet::IpNet>() {
                Ok(n) => nets.push((n, true)),
                Err(_) => eprintln!("[lan] 忽略非法 CIDR: {t}"),
            }
        }
        Self { nets }
    }

    /// 严格构造：任一条目非法即返回 Err。
    ///
    /// 与 [`LanMatcher::new`] 的差异在于**配置写入路径必须用严格模式**——
    /// 静默忽略非法网段会让用户以为已收紧范围，实际却是空集合（全放行或
    /// 全拒绝），属于危险的静默降级。运行时热更新走这里做前置校验。
    pub fn from_cidrs(cidrs: &str) -> Result<Self, String> {
        let mut nets = Vec::new();
        for part in cidrs.split(',') {
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            match t.parse::<ipnet::IpNet>() {
                Ok(n) => nets.push((n, true)),
                Err(_) => return Err(format!("非法网段：{t}")),
            }
        }
        Ok(Self { nets })
    }

    /// 该 IP 是否属于内网白名单。
    pub fn contains(&self, ip: std::net::IpAddr) -> bool {
        // IPv4-mapped IPv6（::ffff:192.168.1.1）统一归一为 IPv4 再匹配
        let ip = match ip {
            std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => std::net::IpAddr::V4(v4),
                None => std::net::IpAddr::V6(v6),
            },
            v4 => v4,
        };
        self.nets.iter().any(|(net, _)| net.contains(&ip))
    }
}

/// 规范化加密策略字符串。非法值返回 None。
pub fn normalize_crypto_policy(v: &str) -> Option<String> {
    match v.trim().to_lowercase().as_str() {
        "auto" => Some("auto".into()),
        "always" | "on" | "1" | "true" | "yes" => Some("always".into()),
        "never" | "off" | "0" | "false" | "no" => Some("never".into()),
        _ => None,
    }
}

/// 当前是否运行在 OpenWrt 集成模式。
///
/// 判定优先级：
/// 1. 环境变量 `TAYGEDO_EDITION`：`openwrt` 或 `standalone`（显式指定，供
///    init.d 与测试使用；跨平台脚本不依赖绝对路径，仅作为版本标识）；
/// 2. 回落到平台标记文件 `/etc/openwrt_release`（OpenWrt 固件固定存在）。
///
/// 结果在进程内缓存：环境变量与固件标识在运行期不会变化。
///
/// OpenWrt 集成模式的含义：无内置 WebUI、无浏览器侧加密前端，LuCI 是唯一
/// 入口；数据由 rpcd/ucode 内部通道调用本服务取得，鉴权交给 OpenWrt
/// （LuCI/ubus 会话）。
pub fn is_openwrt() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| match edition_from_env() {
        Some(v) => v,
        None => std::path::Path::new("/etc/openwrt_release").exists(),
    })
}

/// 解析 `TAYGEDO_EDITION`。无法识别的取值视为未设置（回落到平台标记探测）。
fn edition_from_env() -> Option<bool> {
    let v = std::env::var("TAYGEDO_EDITION").ok()?;
    match v.trim().to_lowercase().as_str() {
        "openwrt" => Some(true),
        "standalone" | "generic" | "linux" | "windows" => Some(false),
        _ => None,
    }
}

/// OpenWrt 集成模式下是否要求 LuCI 会话校验（默认开启）。
///
/// 关闭（`TAYGEDO_LUCI_SESSION_CHECK=0`）仅作为排障降级路径：此时回落到
/// "仅受信来源（回环/内网白名单）放行"，不再校验 ubus 会话。
pub fn luci_session_required() -> bool {
    if !is_openwrt() {
        return false;
    }
    std::env::var("TAYGEDO_LUCI_SESSION_CHECK")
        .map(|v| parse_bool_env(&v))
        .unwrap_or(true)
}

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_bool_env(v: &str) -> bool {
    matches!(
        v.trim().to_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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
    // id → 索引，避免内层循环 O(n²)
    let mut updated_by_id: HashMap<String, usize> = HashMap::new();
    for (i, a) in updated_accounts.iter().enumerate() {
        updated_by_id.insert(a.id.clone(), i);
    }

    for account in &accounts {
        if let Some(ids) = only {
            if !ids.contains(&account.id) {
                continue;
            }
        }
        let already = state_map
            .get(&account.id)
            .map(|d| d == &today)
            .unwrap_or(false);
        if !force && already {
            state.push_log(
                "info",
                format!(
                    "账号 {}（{}）：今日已成功签到，跳过",
                    account.name, account.id
                ),
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

        state.push_log(
            "info",
            format!("账号 {}（{}）：开始签到", account.name, account.id),
        );
        let mut cb = |m: String| {
            state.push_log("info", m);
        };
        match runner::run_account(&state.api, account, Some(&credential_key), &opts, &mut cb).await
        {
            Ok((updated, res)) => {
                if let Some(&idx) = updated_by_id.get(account.id.as_str()) {
                    updated_accounts[idx] = updated;
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
    if let Err(e) = state.store.save_accounts(&updated_accounts) {
        state.push_log("error", format!("保存账号数据失败: {e}"));
    }
    *state.state.write().await = new_state.clone();
    if let Err(e) = state.store.save_state(&new_state) {
        state.push_log("error", format!("保存签到状态失败: {e}"));
    }

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
        format!(
            "签到结束：成功 {}，失败 {}，跳过 {}",
            success_count, failed_count, skipped_count
        ),
    );

    result
}

/// 验证码设备身份有效期（秒）。
const PENDING_DEVICE_TTL: i64 = 600;
/// 同一手机号验证码重发冷却（秒）。
const CAPTCHA_COOLDOWN_SECS: i64 = 60;

/// 发送短信验证码。
pub async fn send_code(state: &AppState, phone: &str) -> Result<(), String> {
    let now = now_unix();
    // 冷却 + 过期清理
    {
        let mut pending = state.pending_devices.lock().unwrap();
        pending.retain(|_, (_, sent_at)| now - *sent_at < PENDING_DEVICE_TTL);
        if let Some((_, sent_at)) = pending.get(phone) {
            let remaining = CAPTCHA_COOLDOWN_SECS - (now - *sent_at);
            if remaining > 0 {
                return Err(format!("验证码发送过于频繁，请 {remaining} 秒后重试"));
            }
        }
    }

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
        .insert(phone.to_string(), (identity, now));
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
        let mut pending = state.pending_devices.lock().unwrap();
        let now = now_unix();
        match pending.remove(phone) {
            Some((dev, sent_at)) if now - sent_at < PENDING_DEVICE_TTL => dev,
            Some(_) => return Err("验证码已过期，请重新发送".to_string()),
            None => return Err("请先发送验证码".to_string()),
        }
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
    let existing = accounts
        .iter()
        .position(|a| a.phone.as_deref() == Some(phone));
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
    state.store.save_accounts(&accounts).unwrap_or_else(|e| {
        eprintln!("[service] 保存账号数据失败: {e}");
    });

    Ok(account)
}

/// 删除账号。同步清理 state.json 签到记录与 config.schedules 定时配置，避免残留幽灵数据。
pub async fn delete_account(state: &AppState, id: &str) -> bool {
    let mut accounts = state.accounts.write().await;
    let before = accounts.len();
    accounts.retain(|a| a.id != id);
    let changed = accounts.len() != before;
    if changed {
        if let Err(e) = state.store.save_accounts(&accounts) {
            state.push_log("error", format!("保存账号数据失败: {e}"));
        }
        // 清理签到状态与定时配置
        let mut state_map = state.state.write().await;
        if state_map.remove(id).is_some() {
            if let Err(e) = state.store.save_state(&state_map) {
                state.push_log("error", format!("保存签到状态失败: {e}"));
            }
        }
        let mut config = state.config.write().await;
        if config.schedules.remove(id).is_some() {
            if let Err(e) = state.store.save_config(&config) {
                state.push_log("error", format!("保存配置失败: {e}"));
            }
        }
        state.push_log("info", format!("已删除账号 {id}（含签到记录与定时配置）"));
    }
    changed
}

/// 设置账号签到时间。
pub async fn set_schedule(state: &AppState, id: &str, time: Option<&str>) -> Result<(), String> {
    if let Some(t) = time {
        if !crate::time::valid_hhmm(t) {
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
    state.store.save_config(&config)?;
    Ok(())
}
