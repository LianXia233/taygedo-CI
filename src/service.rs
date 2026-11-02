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
    /// 登录会话：token -> (过期 unix 秒, 所属用户名)。
    ///
    /// 仅 `webui` 形态存在——OpenWrt 形态由 LuCI/ubus 会话负责身份，
    /// 本服务不签发任何自有凭据，因此连这张表都不该被编译进来。
    #[cfg(feature = "webui")]
    pub sessions: std::sync::Mutex<HashMap<String, (i64, String)>>,
    /// WebUI 登录失败计数：username -> (失败次数, 锁定截止 unix 秒)。防暴力破解。
    #[cfg(feature = "webui")]
    pub login_fails: std::sync::Mutex<HashMap<String, (u32, i64)>>,
    /// 免鉴权模式：为 true 时所有 API 无需登录即可访问。
    ///
    /// **仅 `openwrt` 形态存在**，且只在 LuCI 会话校验被显式关闭（排障降级）
    /// 时作为兼容路径生效；`webui` 形态恒为 false——独立部署有登录，
    /// 免鉴权等于把业务接口完全敞开，不提供该开关。
    #[cfg(feature = "openwrt")]
    pub no_auth: bool,
    /// 应用层加密会话管理器（`webui` 形态的 HTTP 信封通道）。
    ///
    /// OpenWrt 形态不挂加密中间件，但身份密钥仍随 `keyring.json` 加载
    /// （存储加密与身份指纹依赖它），因此字段保留。
    pub crypto: CryptoManager,
    /// 启动时生成的初始随机口令（**仅保存在内存**，用于单次提示，不落盘、不进日志）。
    #[cfg(feature = "webui")]
    pub initial_password: Option<String>,
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
        // 内网免鉴权放行开关覆盖（仅 OpenWrt 排障路径生效）
        if let Ok(v) = std::env::var("TAYGEDO_LAN_NO_AUTH") {
            config.lan_no_auth = parse_bool_env(&v);
        }

        // 首次启动若没有凭据密钥，立即落盘
        if config.credential_key.is_empty() {
            config.credential_key = crate::crypto::generate_credential_key();
        }

        // 用户名初始化（默认 admin）。仅 `webui` 形态存在自有账号体系。
        #[cfg(feature = "webui")]
        if config
            .web_username
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
        {
            config.web_username = Some("admin".into());
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

        // ---- 形态专属初始化 ----
        //
        // `webui` 形态：建立自有口令体系。
        // - 设置了 TAYGEDO_WEB_PASSWORD 时**以它为准**（幂等覆盖）：这是忘记
        //   口令时唯一可用的恢复路径，也让自动化部署能固定凭据；
        // - 未设置且首次启动：生成随机口令，杜绝默认 admin。
        //
        // `openwrt` 形态：**不生成、不迁移任何自有口令**——入口是 LuCI，
        // 鉴权交给 ubus 会话；一个无处可用的口令只会扩大可爆破面。
        #[cfg(feature = "webui")]
        let mut initial_password: Option<String> = None;
        #[cfg(feature = "webui")]
        let mut env_password_applied = false;
        #[cfg(feature = "webui")]
        {
            let env_pwd = std::env::var("TAYGEDO_WEB_PASSWORD")
                .ok()
                .filter(|s| !s.trim().is_empty());
            // 当前存储的口令是否已与显式环境变量一致（一致就不必重写，避免
            // 每次启动都换盐、把日志刷满）
            let stored_matches = match (
                &config.web_password_hash,
                &config.web_password_salt,
                env_pwd.as_deref(),
            ) {
                (Some(h), Some(s), Some(p))
                    if config
                        .web_password_version
                        .unwrap_or(crate::crypto::PWD_HASH_V1)
                        >= crate::crypto::PWD_HASH_V2 =>
                {
                    let computed = crate::crypto::hash_password_v2(p, s);
                    crate::crypto::constant_time_eq(computed.as_bytes(), h.as_bytes())
                }
                _ => false,
            };
            let had_password = config.web_password_hash.is_some();
            match env_pwd {
                Some(p) if !stored_matches => {
                    let salt = crate::crypto::random_hex(16);
                    config.web_password_hash = Some(crate::crypto::hash_password_v2(&p, &salt));
                    config.web_password_salt = Some(salt);
                    config.web_password_version = Some(crate::crypto::PWD_HASH_V2);
                    config.web_password_must_change = false;
                    // 首次启动就由环境变量给定时不算"重置"，只是初始化
                    env_password_applied = had_password;
                }
                Some(_) => {
                    // 已一致：保留既有 hash/盐，不写盘
                }
                None if !had_password => {
                    let password = crate::crypto::generate_passphrase(16);
                    let salt = crate::crypto::random_hex(16);
                    config.web_password_hash =
                        Some(crate::crypto::hash_password_v2(&password, &salt));
                    config.web_password_salt = Some(salt);
                    config.web_password_version = Some(crate::crypto::PWD_HASH_V2);
                    config.web_password_must_change = true;
                    initial_password = Some(password);
                }
                None => {}
            }
        }
        // 兼容历史配置：v1（单轮 sha256 + 8 字节盐）在口令校验通过后就地升级为 v2
        #[cfg(feature = "webui")]
        if config.web_password_version.is_none() {
            config.web_password_version = Some(crate::crypto::PWD_HASH_V1);
        }
        store.save_config(&config)?;

        // 免鉴权模式：环境变量 TAYGEDO_NO_AUTH=1/true/yes/on 时开启。
        //
        // **仅 `openwrt` 形态存在该开关**，而且只有 LuCI 会话校验被显式关闭
        // （排障降级）时才作为兼容路径生效。`webui` 形态有登录页，免鉴权等于
        // 把业务接口完全敞开，因此不提供——连分支都不编译。
        #[cfg(feature = "openwrt")]
        let session_auth = luci_session_required();
        #[cfg(feature = "openwrt")]
        let no_auth = std::env::var("TAYGEDO_NO_AUTH")
            .map(|v| parse_bool_env(&v))
            .unwrap_or(false)
            && !session_auth;

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
            #[cfg(feature = "webui")]
            sessions: std::sync::Mutex::new(HashMap::new()),
            #[cfg(feature = "webui")]
            login_fails: std::sync::Mutex::new(HashMap::new()),
            #[cfg(feature = "openwrt")]
            no_auth,
            crypto,
            #[cfg(feature = "webui")]
            initial_password: initial_password.clone(),
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
        // 访问控制方式与初始凭据提示（按形态分别编译，互不引用对方的变量）。
        #[cfg(feature = "webui")]
        if initial_password.is_some() {
            // 明文口令**不写入日志缓冲区**（日志可经 /api/logs 读取），
            // 只在启动横幅中出现一次，供操作者记录。
            app.push_log(
                "warn",
                "已初始化 WebUI 登录账号 admin，口令为本次启动随机生成，\
                 请在启动横幅中查看并在首次登录后立即修改。"
                    .into(),
            );
        }
        #[cfg(feature = "webui")]
        if env_password_applied {
            app.push_log(
                "warn",
                "已按环境变量 TAYGEDO_WEB_PASSWORD 重置 WebUI 登录口令（该变量优先级最高，\
                 不设置它才会沿用 UI 中修改过的口令）"
                    .into(),
            );
        }
        #[cfg(feature = "webui")]
        app.push_log(
            "info",
            "形态 webui：内置 WebUI 已启用，业务接口需 Bearer token".into(),
        );

        #[cfg(feature = "openwrt")]
        if session_auth {
            app.push_log(
                "info",
                "形态 openwrt：LuCI 为唯一入口，鉴权由 OpenWrt（LuCI/ubus）会话负责；\
                 仅接受受信内网来源"
                    .into(),
            );
        } else if no_auth {
            app.push_log(
                "warn",
                "免鉴权模式已开启（OpenWrt 排障专享）：API 无需登录即可访问，仅限内网使用".into(),
            );
        } else {
            app.push_log(
                "warn",
                "LuCI 会话校验已关闭（排障降级）：业务接口仅按来源放行，请尽快恢复".into(),
            );
        }
        Ok(app)
    }

    /// 判定来源是否可享受免鉴权放行。
    ///
    /// 仅 `openwrt` 形态存在：`webui` 形态有登录页，不存在"免鉴权"语义。
    ///
    /// 即使 `no_auth` 已开启，非内网来源也不放行——避免"免鉴权开关被打开后
    /// 服务被直接暴露到公网"这一配置失误演变为完全开放。
    #[cfg(feature = "openwrt")]
    pub async fn no_auth_allows(&self, peer_ip: Option<std::net::IpAddr>) -> bool {
        if !self.no_auth {
            return false;
        }
        let lan_ok = self.config.read().await.lan_no_auth;
        if !lan_ok {
            // 未启用 LAN 白名单时，按原语义放行全部来源（保持向后兼容）
            return true;
        }
        match peer_ip {
            Some(ip) => self.lan_matches(ip),
            // 无法解析来源（如 Unix socket / 代理未透传）时保守放行：
            // 免鉴权模式本身即为内网自用设计，此处不额外收紧以免破坏既有部署。
            None => true,
        }
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

/// 自有登录体系（仅 `webui` 形态编译）。
///
/// OpenWrt 形态的身份由 LuCI/ubus 会话承担：本服务不签发 token、不校验口令、
/// 不做失败锁定，因此这整块能力（以及与它配套的状态字段）不进入 OpenWrt 产物。
/// 这不是"运行时关闭"，而是编译期不存在。
#[cfg(feature = "webui")]
impl AppState {
    /// 校验登录账号密码。
    ///
    /// 按 `web_password_version` 选择哈希算法：v2（scrypt）为当前格式，
    /// v1（单轮 sha256）仅用于兼容历史配置。两者均使用恒定时间比较。
    pub async fn verify_login(&self, username: &str, password: &str) -> bool {
        let config = self.config.read().await;
        let expected_user = config.web_username.as_deref().unwrap_or("admin");
        // 用户名比较也走恒定时间，避免枚举有效用户名
        if !crate::crypto::constant_time_eq(username.as_bytes(), expected_user.as_bytes()) {
            return false;
        }
        match (&config.web_password_hash, &config.web_password_salt) {
            (Some(hash), Some(salt)) => {
                let version = config
                    .web_password_version
                    .unwrap_or(crate::crypto::PWD_HASH_V1);
                let computed = if version >= crate::crypto::PWD_HASH_V2 {
                    crate::crypto::hash_password_v2(password, salt)
                } else {
                    crate::crypto::hash_password(password, salt)
                };
                crate::crypto::constant_time_eq(computed.as_bytes(), hash.as_bytes())
            }
            _ => false,
        }
    }

    /// 历史 v1 口令校验通过后就地升级为 v2。
    ///
    /// 仅在**校验成功**后调用，因此不会把错误口令固化为新哈希。
    pub async fn upgrade_password_hash_if_legacy(&self, password: &str) -> bool {
        let needs_upgrade = {
            let config = self.config.read().await;
            config
                .web_password_version
                .unwrap_or(crate::crypto::PWD_HASH_V1)
                < crate::crypto::PWD_HASH_V2
        };
        if !needs_upgrade {
            return false;
        }
        let mut config = self.config.write().await;
        let salt = crate::crypto::random_hex(16);
        config.web_password_hash = Some(crate::crypto::hash_password_v2(password, &salt));
        config.web_password_salt = Some(salt);
        config.web_password_version = Some(crate::crypto::PWD_HASH_V2);
        if let Err(e) = self.store.save_config(&config) {
            self.push_log("error", format!("升级口令哈希失败: {e}"));
            return false;
        }
        true
    }

    /// 修改登录账号与口令。成功后**吊销全部登录会话**。
    ///
    /// `keep_crypto_sid`：发起本次改密的加密会话号。加密会话与登录会话是
    /// 两套独立凭据，但改密必须让**所有**已协商的信道失效（否则旧客户端
    /// 仍能继续用旧密钥通信）。唯一的例外是当前这条：它还要负责把
    /// "改密成功"的响应加密送回去，销毁它会让客户端收到 428 而误判失败。
    pub async fn set_credentials(
        &self,
        username: &str,
        new_password: &str,
        keep_crypto_sid: Option<&str>,
    ) -> Result<(), String> {
        {
            let mut config = self.config.write().await;
            if !username.trim().is_empty() {
                config.web_username = Some(username.trim().to_string());
            }
            let salt = crate::crypto::random_hex(16);
            config.web_password_hash = Some(crate::crypto::hash_password_v2(new_password, &salt));
            config.web_password_salt = Some(salt);
            config.web_password_version = Some(crate::crypto::PWD_HASH_V2);
            // 使用者已自行设置口令，撤销"必须修改口令"状态
            config.web_password_must_change = false;
            self.store.save_config(&config)?;
        }
        // 口令变更后立即吊销全部既有会话，堵住"凭据已泄露但旧 token 仍有效"的窗口
        let revoked = self.revoke_all_sessions();
        // 加密会话同样作废（保留发起本次请求的那条，见函数文档）
        let crypto_revoked = match keep_crypto_sid {
            Some(sid) => self.crypto.destroy_all_except(sid),
            None => self.crypto.destroy_all(),
        };
        self.push_log(
            "info",
            format!("登录口令已更新，已吊销 {revoked} 个会话 / {crypto_revoked} 个加密会话"),
        );
        Ok(())
    }

    /// 签发登录 token（有效期 7 天）。token 与该用户名绑定，便于按用户吊销。
    pub fn issue_token(&self, username: &str) -> String {
        let token = crate::crypto::random_hex(32);
        let expiry = now_unix() + SESSION_TTL_SECS;
        self.sessions
            .lock()
            .unwrap()
            .insert(token.clone(), (expiry, username.to_string()));
        token
    }

    /// 校验 token，返回所属用户名。
    pub fn validate_token(&self, token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        // 形态前置校验：拒绝非法长度，减少无谓的锁竞争
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let now = now_unix();
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, (exp, _)| *exp > now);
        sessions.get(token).map(|(_, user)| user.clone())
    }

    /// 吊销单个会话。
    pub fn revoke_session(&self, token: &str) -> bool {
        self.sessions.lock().unwrap().remove(token).is_some()
    }

    /// 吊销全部登录会话，返回吊销数量。
    pub fn revoke_all_sessions(&self) -> usize {
        let mut s = self.sessions.lock().unwrap();
        let n = s.len();
        s.clear();
        n
    }

    /// 当前有效会话数（诊断用；顺带清理过期条目）。
    #[allow(dead_code)]
    pub fn active_sessions(&self) -> usize {
        let now = now_unix();
        let mut s = self.sessions.lock().unwrap();
        s.retain(|_, (exp, _)| *exp > now);
        s.len()
    }

    /// 是否需要强制修改口令（初始为随机口令时为真）。
    pub async fn must_change_password(&self) -> bool {
        self.config.read().await.web_password_must_change
    }

    /// 是否处于登录锁定。返回 Some(剩余秒数) 表示已锁定。
    pub fn login_lock_remaining(&self, username: &str) -> Option<i64> {
        let now = now_unix();
        let fails = self.login_fails.lock().unwrap();
        match fails.get(username) {
            Some((count, lock_until)) if *count >= MAX_LOGIN_FAILS && *lock_until > now => {
                Some(*lock_until - now)
            }
            _ => None,
        }
    }

    /// 记录一次登录失败；连续失败达到阈值后锁定 5 分钟。
    pub fn record_login_fail(&self, username: &str) {
        let now = now_unix();
        let mut fails = self.login_fails.lock().unwrap();
        let entry = fails.entry(username.to_string()).or_insert((0, 0));
        entry.0 += 1;
        if entry.0 >= MAX_LOGIN_FAILS {
            entry.1 = now + LOGIN_LOCK_SECS;
        }
        if entry.0 > MAX_LOGIN_FAILS * 3 {
            // 防止无限膨胀
            entry.0 = MAX_LOGIN_FAILS;
            entry.1 = now + LOGIN_LOCK_SECS;
        }
    }

    /// 登录成功后清除失败计数。
    pub fn reset_login_fails(&self, username: &str) {
        self.login_fails.lock().unwrap().remove(username);
    }
}

/// WebUI 登录连续失败阈值（仅 `webui` 形态）。
#[cfg(feature = "webui")]
pub const MAX_LOGIN_FAILS: u32 = 5;
/// 达到阈值后的锁定时长（秒，仅 `webui` 形态）。
#[cfg(feature = "webui")]
pub const LOGIN_LOCK_SECS: i64 = 300;
/// 登录会话有效期（秒，7 天，仅 `webui` 形态）。
#[cfg(feature = "webui")]
pub const SESSION_TTL_SECS: i64 = 7 * 24 * 3600;

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

/// 当前构建形态是否为 OpenWrt 集成。
///
/// 这是**编译期常量**，不是运行期探测。形态由 Cargo feature 决定，二进制
/// 一旦产出就不会改变。原先的 `TAYGEDO_EDITION` 环境变量探测与
/// `/etc/openwrt_release` 文件判定已经删除——那会让同一个二进制在两种形态
/// 之间摇摆：既无法裁剪代码与前端资产，也让"OpenWrt 上没有 WebUI"变成一句
/// 依赖部署者正确设置环境变量的约定。
#[cfg_attr(feature = "openwrt", allow(dead_code))]
pub const IS_OPENWRT: bool = cfg!(feature = "openwrt");

/// 当前构建形态标识（`openwrt` / `webui`），供 `/api/meta` 与日志使用。
pub const EDITION: &str = if cfg!(feature = "openwrt") {
    "openwrt"
} else {
    "webui"
};

/// 是否要求 LuCI 会话校验（仅 `openwrt` 形态）。
///
/// 关闭（`TAYGEDO_LUCI_SESSION_CHECK=0`）仅作为排障降级路径：此时回落到
/// "仅受信来源（回环/内网白名单）放行"，不再校验 ubus 会话。
#[cfg(feature = "openwrt")]
pub fn luci_session_required() -> bool {
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
