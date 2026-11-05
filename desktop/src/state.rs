//! 应用状态与全部业务动作。
//!
//! 视图层只负责"怎么画"和"把输入框里的值取出来"，所有网络动作、弹窗流转、
//! Toast 生命周期、轮询节奏都在这里，行为逐条对齐 WebUI 的 `src/ui.html`。
//!
//! 网络命令一律投递到独立 tokio runtime，结果经 oneshot 回传后由 GPUI 的
//! `Context::spawn` 在 UI 线程写回 Entity 并 `cx.notify()`，不阻塞 UI 线程。

use std::future::Future;
use std::time::Duration;

use gpui::Context;
use serde_json::json;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::client::{
    AccountRow, AccountsResponse, AddAccountRequest, ApiClient, ApiError, ConfigResponse, LogRow,
};
use crate::theme::{DARK, LIGHT, Palette};

/// 与本机回环服务的连接状态。桌面单包没有"登录态"，只有"服务起没起来"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connected,
    Failed(String),
}

/// 当前显示的弹窗。同一时刻只允许一个（与 WebUI 的 modal-mask 语义一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    None,
    /// 添加账号：密码 / 验证码两种模式切换。
    AddAccount,
    /// 全局设置。
    Settings,
    /// 删除确认：WebUI 用 `confirm()`，桌面端用等价的自绘弹窗。
    ConfirmDelete {
        id: String,
        label: String,
    },
}

/// Toast 语气，对应 WebUI 的 `.toast.ok` / `.toast.err`。
///
/// 只有两种：桌面端的提示要么是操作成功的确认，要么是失败原因。WebUI 里那条
/// 中性提示（默认 `.toast`）在桌面流程中没有调用点，因此不引入第三种语气。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Ok,
    Err,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    /// 自增序号：用于让"后一条 Toast 覆盖前一条"的定时清除不会误清新的。
    pub seq: u64,
}

/// 添加账号的登录方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    Password,
    Captcha,
}

/// 添加账号表单的提交载荷（值由视图从输入框读出后传入）。
#[derive(Debug, Clone)]
pub struct AddPayload {
    pub phone: String,
    pub mode: LoginMode,
    pub password: String,
    pub captcha: String,
    pub name: String,
}

/// 全局设置表单的提交载荷。
#[derive(Debug, Clone)]
pub struct SettingsPayload {
    pub default_schedule: String,
    pub coin_tasks: bool,
    pub cloud_duration: bool,
    pub share_platform: String,
}

pub struct AppState {
    pub client: ApiClient,
    pub handle: Handle,
    pub base_url: String,

    pub connection: Connection,
    /// 是否有网络命令在执行（用于按钮禁用与"处理中"提示）。
    pub busy: bool,
    /// 正在签到的账号 id：只禁用该行按钮，与 WebUI 的单按钮 loading 一致。
    pub signing_in: Option<String>,

    pub accounts: Vec<AccountRow>,
    pub today: String,
    pub logs: Vec<LogRow>,
    /// 日志筛选：`None` 表示「全部」，`Some(id)` 表示只看该账号。
    pub log_filter: Option<String>,
    pub config: Option<ConfigResponse>,

    pub modal: Modal,
    pub toast: Option<Toast>,
    toast_seq: u64,
    /// 验证码倒计时（秒）。>0 时按钮显示 `Ns` 并禁用，对齐 WebUI 的 60 秒。
    pub code_countdown: u64,

    pub dark: bool,
}

impl AppState {
    pub fn new(
        client: ApiClient,
        handle: Handle,
        base_url: String,
        startup_error: Option<String>,
    ) -> Self {
        let connection = match startup_error {
            Some(message) => Connection::Failed(message),
            None => Connection::Connected,
        };
        Self {
            client,
            handle,
            base_url,
            connection,
            busy: false,
            signing_in: None,
            accounts: Vec::new(),
            today: String::new(),
            logs: Vec::new(),
            log_filter: None,
            config: None,
            modal: Modal::None,
            toast: None,
            toast_seq: 0,
            code_countdown: 0,
            dark: false,
        }
    }

    pub fn palette(&self) -> &'static Palette {
        if self.dark { &DARK } else { &LIGHT }
    }

    pub fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        cx.notify();
    }

    // ---- 统计卡片 ----

    /// 返回 (总账号, 今日已签到, 待签到)，对应 WebUI 的三个 `.stat`。
    pub fn stats(&self) -> (usize, usize, usize) {
        let total = self.accounts.len();
        let done = self.accounts.iter().filter(|a| a.signed_today).count();
        (total, done, total.saturating_sub(done))
    }

    // ---- 弹窗流转 ----

    pub fn open_add(&mut self, cx: &mut Context<Self>) {
        self.modal = Modal::AddAccount;
        cx.notify();
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.modal = Modal::Settings;
        cx.notify();
    }

    pub fn ask_delete(&mut self, cx: &mut Context<Self>, id: String, label: String) {
        self.modal = Modal::ConfirmDelete { id, label };
        cx.notify();
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal = Modal::None;
        cx.notify();
    }

    // ---- Toast ----

    fn push_toast(&mut self, cx: &mut Context<Self>, text: impl Into<String>, kind: ToastKind) {
        self.toast_seq += 1;
        let seq = self.toast_seq;
        self.toast = Some(Toast {
            text: text.into(),
            kind,
            seq,
        });
        cx.notify();
        // WebUI 的 toast 是 2600ms 自动消失；这里保持同一节奏。
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2600))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.toast.as_ref().map(|t| t.seq) == Some(seq) {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ---- 数据加载 ----

    /// 首屏：一次性拉 meta / 账号 / 日志 / 配置（对齐 WebUI 的 init()）。
    pub fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.execute(
            cx,
            async move {
                let meta = client.meta().await?;
                let accounts = client.list_accounts().await?;
                let logs = client.logs(200).await?;
                let config = client.config().await?;
                Ok::<_, ApiError>((meta, accounts, logs, config))
            },
            |this, result, cx| match result {
                Ok((_meta, accounts, logs, config)) => {
                    this.apply_accounts(accounts);
                    this.logs = logs.logs;
                    this.config = Some(config);
                    this.connection = Connection::Connected;
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    /// 手动刷新账号与日志（页头刷新按钮）。
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.execute(
            cx,
            async move {
                let accounts = client.list_accounts().await?;
                let logs = client.logs(200).await?;
                Ok::<_, ApiError>((accounts, logs))
            },
            |this, result, cx| match result {
                Ok((accounts, logs)) => {
                    this.apply_accounts(accounts);
                    this.logs = logs.logs;
                    this.connection = Connection::Connected;
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    /// 只刷新账表（轮询用，不覆盖日志面板的滚动位置）。
    fn refresh_accounts_silent(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let (tx, rx) = oneshot::channel();
            let handle = this.read_with(&*cx, |state, _| state.handle.clone()).ok();
            if let Some(handle) = handle {
                handle.spawn(async move {
                    let _ = tx.send(client.list_accounts().await);
                });
                if let Ok(Ok(response)) = rx.await {
                    let _ = this.update(cx, |this, cx| {
                        this.apply_accounts(response);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// 只刷新日志（轮询用，对齐 WebUI 每 3 秒的 loadLogs）。
    fn refresh_logs_silent(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let (tx, rx) = oneshot::channel();
            let handle = this.read_with(&*cx, |state, _| state.handle.clone()).ok();
            if let Some(handle) = handle {
                handle.spawn(async move {
                    let _ = tx.send(client.logs(200).await);
                });
                if let Ok(Ok(response)) = rx.await {
                    let _ = this.update(cx, |this, cx| {
                        this.logs = response.logs;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    // ---- 业务动作 ----

    /// 立即签到（单账号）。与 WebUI 一致：完成后同时刷新账号与日志。
    pub fn signin(&mut self, cx: &mut Context<Self>, id: String) {
        if self.signing_in.is_some() {
            return;
        }
        self.signing_in = Some(id.clone());
        let client = self.client.clone();
        self.execute(
            cx,
            async move {
                let result = client.signin(&id, true).await?;
                let accounts = client.list_accounts().await?;
                let logs = client.logs(200).await?;
                Ok::<_, ApiError>((result, accounts, logs))
            },
            |this, result, cx| {
                this.signing_in = None;
                match result {
                    Ok((result, accounts, logs)) => {
                        this.apply_accounts(accounts);
                        this.logs = logs.logs;
                        this.connection = Connection::Connected;
                        let summary = result
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .unwrap_or("签到完成")
                            .to_string();
                        this.push_toast(cx, summary, ToastKind::Ok);
                    }
                    Err(error) => this.fail(cx, error),
                }
            },
        );
    }

    pub fn set_schedule(&mut self, cx: &mut Context<Self>, id: String, time: String) {
        let value = time.trim().to_string();
        let client = self.client.clone();
        self.execute(
            cx,
            async move {
                let payload = if value.is_empty() {
                    None
                } else {
                    Some(value.as_str())
                };
                client.set_schedule(&id, payload).await?;
                client.list_accounts().await
            },
            |this, result, cx| match result {
                Ok(accounts) => {
                    this.apply_accounts(accounts);
                    this.push_toast(cx, "签到时间已更新", ToastKind::Ok);
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    pub fn delete_account(&mut self, cx: &mut Context<Self>, id: String) {
        self.modal = Modal::None;
        let client = self.client.clone();
        // `id` 要同时给两处：异步协程里发请求，以及结果回调里复位日志筛选。
        // 协程按值捕获，所以另拷一份，避免 moved value。
        let target = id.clone();
        self.execute(
            cx,
            async move {
                client.delete_account(&target).await?;
                client.list_accounts().await
            },
            move |this, result, cx| match result {
                Ok(accounts) => {
                    this.apply_accounts(accounts);
                    // 被删账号的日志筛选要复位，否则会停在一个不存在的条目上。
                    if this.log_filter.as_deref() == Some(id.as_str()) {
                        this.log_filter = None;
                    }
                    this.push_toast(cx, "已删除", ToastKind::Ok);
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    /// 下发短信验证码；成功后启动 60 秒倒计时（对齐 WebUI 的按钮倒计时）。
    pub fn send_code(&mut self, cx: &mut Context<Self>, phone: String) {
        if self.code_countdown > 0 {
            return;
        }
        let phone = phone.trim().to_string();
        if phone.is_empty() {
            self.push_toast(cx, "请输入手机号", ToastKind::Err);
            return;
        }
        let client = self.client.clone();
        self.execute(
            cx,
            async move { client.send_code(&phone).await },
            |this, result, cx| match result {
                Ok(_) => {
                    this.push_toast(cx, "验证码已发送", ToastKind::Ok);
                    this.code_countdown = 60;
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    /// 提交添加账号。
    pub fn submit_add(&mut self, cx: &mut Context<Self>, payload: AddPayload) {
        let phone = payload.phone.trim().to_string();
        if phone.is_empty() {
            self.push_toast(cx, "请输入手机号", ToastKind::Err);
            return;
        }
        let request = match payload.mode {
            LoginMode::Password => {
                if payload.password.is_empty() {
                    self.push_toast(cx, "请输入密码", ToastKind::Err);
                    return;
                }
                AddAccountRequest {
                    phone,
                    mode: "password".to_string(),
                    password: Some(payload.password),
                    captcha: None,
                    name: {
                        let n = payload.name.trim();
                        if n.is_empty() {
                            None
                        } else {
                            Some(n.to_string())
                        }
                    },
                }
            }
            LoginMode::Captcha => {
                if payload.captcha.trim().is_empty() {
                    self.push_toast(cx, "请输入验证码", ToastKind::Err);
                    return;
                }
                AddAccountRequest {
                    phone,
                    mode: "captcha".to_string(),
                    password: None,
                    captcha: Some(payload.captcha.trim().to_string()),
                    name: {
                        let n = payload.name.trim();
                        if n.is_empty() {
                            None
                        } else {
                            Some(n.to_string())
                        }
                    },
                }
            }
        };

        let client = self.client.clone();
        self.execute(
            cx,
            async move {
                client.add_account(&request).await?;
                client.list_accounts().await
            },
            |this, result, cx| match result {
                Ok(accounts) => {
                    this.apply_accounts(accounts);
                    this.modal = Modal::None;
                    this.push_toast(cx, "登录成功", ToastKind::Ok);
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    /// 保存全局设置（默认签到时间 / 金币任务 / 云异环时长 / 分享平台）。
    pub fn save_settings(&mut self, cx: &mut Context<Self>, payload: SettingsPayload) {
        let schedule = payload.default_schedule.trim().to_string();
        if !schedule.is_empty() && !is_valid_hhmm(&schedule) {
            self.push_toast(cx, "签到时间格式应为 HH:MM", ToastKind::Err);
            return;
        }
        let share = payload.share_platform.trim().to_string();
        let patch = json!({
            "default_schedule": schedule,
            "coin_tasks": payload.coin_tasks,
            "cloud_duration": payload.cloud_duration,
            "share_platform": if share.is_empty() { "qq".to_string() } else { share },
        });
        let client = self.client.clone();
        self.execute(
            cx,
            async move {
                client.update_config(&patch).await?;
                let config = client.config().await?;
                let accounts = client.list_accounts().await?;
                Ok::<_, ApiError>((config, accounts))
            },
            |this, result, cx| match result {
                Ok((config, accounts)) => {
                    this.config = Some(config);
                    this.apply_accounts(accounts);
                    this.modal = Modal::None;
                    this.push_toast(cx, "设置已保存", ToastKind::Ok);
                }
                Err(error) => this.fail(cx, error),
            },
        );
    }

    // ---- 日志筛选 ----

    pub fn select_filter(&mut self, cx: &mut Context<Self>, filter: Option<String>) {
        self.log_filter = filter;
        cx.notify();
    }

    /// 按当前筛选返回日志。
    ///
    /// 与 WebUI 的实现一致：不做服务端过滤，而是用该账号的 name / id / uid
    /// 三个关键词对 `message` 做子串匹配（日志里并不带结构化账号字段）。
    pub fn visible_logs(&self) -> Vec<&LogRow> {
        match self.log_filter.as_deref() {
            None => self.logs.iter().collect(),
            Some(id) => {
                let Some(account) = self.accounts.iter().find(|a| a.id == id) else {
                    return self.logs.iter().collect();
                };
                let keywords: Vec<&str> = [
                    Some(account.name.as_str()),
                    Some(account.id.as_str()),
                    Some(account.uid.as_str()),
                ]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
                self.logs
                    .iter()
                    .filter(|log| keywords.iter().any(|k| log.message.contains(k)))
                    .collect()
            }
        }
    }

    // ---- 轮询（对齐 WebUI：日志 3 秒、账号 10 秒、验证码倒计时 1 秒） ----

    pub fn start_polling(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut tick: u64 = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                tick += 1;
                let _ = this.update(cx, |state, cx| {
                    let mut changed = false;
                    if state.code_countdown > 0 {
                        state.code_countdown -= 1;
                        changed = true;
                    }
                    if matches!(state.connection, Connection::Connected) {
                        if tick.is_multiple_of(3) {
                            state.refresh_logs_silent(cx);
                        }
                        if tick.is_multiple_of(10) {
                            state.refresh_accounts_silent(cx);
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    // ---- 内部 ----

    /// 通用命令执行器：置忙 -> tokio 线程执行 -> 回写 Entity 并通知重绘。
    fn execute<T, F, A>(&mut self, cx: &mut Context<Self>, fut: F, apply: A)
    where
        T: Send + 'static,
        F: Future<Output = Result<T, ApiError>> + Send + 'static,
        A: FnOnce(&mut Self, Result<T, ApiError>, &mut Context<Self>) + 'static,
    {
        self.busy = true;
        cx.notify();

        let (tx, rx) = oneshot::channel::<Result<T, ApiError>>();
        let handle = self.handle.clone();
        handle.spawn(async move {
            let result = fut.await;
            let _ = tx.send(result);
        });

        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.await {
                let _ = this.update(cx, |this, cx| {
                    this.busy = false;
                    apply(this, result, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn apply_accounts(&mut self, response: AccountsResponse) {
        self.accounts = response.accounts;
        self.today = response.today;
    }

    fn fail(&mut self, cx: &mut Context<Self>, error: ApiError) {
        let message = error.to_string();
        self.signing_in = None;
        self.connection = Connection::Failed(message.clone());
        self.push_toast(cx, message, ToastKind::Err);
    }
}

/// `HH:MM` 校验：WebUI 用 `<input type="time">`，GPUI 没有原生时间控件，
/// 只能收文本，因此在这里做同一语义的格式校验。
fn is_valid_hhmm(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    let hour: u32 = match value[0..2].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let minute: u32 = match value[3..5].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    hour < 24 && minute < 60
}

/// 供视图读取的配置快照（避免视图里散落 `Option` 解包）。
pub fn config_or_default(config: &Option<ConfigResponse>) -> ConfigResponse {
    config.clone().unwrap_or(ConfigResponse {
        default_schedule: String::new(),
        coin_tasks: true,
        cloud_duration: true,
        share_platform: "qq".to_string(),
    })
}

/// 供视图判断某账号是否正在签到。
impl AppState {
    pub fn signing(&self, id: &str) -> bool {
        self.signing_in.as_deref() == Some(id)
    }
}
