//! 应用状态：登录态（此处为连接态）、账号列表、日志、状态消息与加载标记。
//!
//! 所有网络命令都投递到独立的 tokio runtime 执行，结果经 oneshot 回传后由 GPUI
//! 的 `Context::spawn` 在 UI 线程写回 Entity 并 `cx.notify()`，绝不阻塞 UI 线程。

use std::future::Future;
use std::time::Duration;

use gpui::Context;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::oneshot;

use crate::client::{
    AccountRow, AccountsResponse, AddAccountRequest, ApiClient, ApiError, ConfigResponse, LogRow,
    MetaInfo,
};

/// 与本机回环服务的连接状态。桌面形态没有"登录态"，只有"能不能连上"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Connected,
    Failed(String),
}

pub struct AppState {
    pub client: ApiClient,
    pub handle: Handle,
    pub base_url: String,
    pub connection: Connection,
    pub busy: bool,
    pub status: String,
    pub status_error: bool,
    pub accounts: Vec<AccountRow>,
    pub today: String,
    pub logs: Vec<LogRow>,
    pub config: Option<ConfigResponse>,
    pub meta: Option<MetaInfo>,
    pub last_signin: Option<String>,
    /// 自动刷新间隔（秒），可在设置页调整，后台轮询按此值生效。
    pub refresh_secs: u64,
}

impl AppState {
    pub fn new(
        client: ApiClient,
        handle: Handle,
        base_url: String,
        startup_error: Option<String>,
    ) -> Self {
        let (connection, status, status_error) = match startup_error {
            Some(message) => (Connection::Failed(message.clone()), message, true),
            None => (
                Connection::Connecting,
                "正在连接本机回环服务…".to_string(),
                false,
            ),
        };
        Self {
            client,
            handle,
            base_url,
            connection,
            busy: false,
            status,
            status_error,
            accounts: Vec::new(),
            today: String::new(),
            logs: Vec::new(),
            config: None,
            meta: None,
            last_signin: None,
            refresh_secs: 30,
        }
    }

    /// 通用命令执行器：设置加载标记 -> tokio 线程执行 -> 回写 Entity 并通知重绘。
    fn execute<T, F, A>(&mut self, cx: &mut Context<Self>, label: &str, fut: F, apply: A)
    where
        T: Send + 'static,
        F: Future<Output = Result<T, ApiError>> + Send + 'static,
        A: FnOnce(&mut Self, Result<T, ApiError>, &mut Context<Self>) + 'static,
    {
        self.busy = true;
        self.status = format!("{label}…");
        self.status_error = false;
        cx.notify();

        let (tx, rx) = oneshot::channel::<Result<T, ApiError>>();
        let joined = self.handle.spawn(async move {
            let result = fut.await;
            let _ = tx.send(result);
        });
        drop(joined);

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

    /// 启动探活并加载首屏数据（账号、日志、配置、元信息）。
    pub fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.connection = Connection::Connecting;
        self.execute(
            cx,
            "正在连接本机回环服务",
            async move {
                let meta = client.meta().await?;
                let accounts = client.list_accounts().await?;
                let logs = client.logs(200).await?;
                let config = client.config().await?;
                Ok::<_, ApiError>((meta, accounts, logs, config))
            },
            |this, result, _cx| match result {
                Ok((meta, accounts, logs, config)) => {
                    this.meta = Some(meta);
                    this.apply_accounts(accounts);
                    this.logs = logs.logs;
                    this.config = Some(config);
                    this.connection = Connection::Connected;
                    this.status_error = false;
                    this.status = "已连接本机回环服务".to_string();
                }
                Err(error) => this.fail(error),
            },
        );
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在刷新",
            async move {
                let accounts = client.list_accounts().await?;
                let logs = client.logs(200).await?;
                Ok::<_, ApiError>((accounts, logs))
            },
            |this, result, _cx| match result {
                Ok((accounts, logs)) => {
                    this.apply_accounts(accounts);
                    this.logs = logs.logs;
                    this.connection = Connection::Connected;
                    this.status_error = false;
                    this.status = "数据已刷新".to_string();
                }
                Err(error) => this.fail(error),
            },
        );
    }

    /// 手动签到：执行后顺带刷新账号列表与日志，使"今日已签"即时更新。
    pub fn signin(&mut self, cx: &mut Context<Self>, id: String) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在执行签到",
            async move {
                let result = client.signin(&id, true).await?;
                let accounts = client.list_accounts().await?;
                let logs = client.logs(200).await?;
                Ok::<_, ApiError>((result, accounts, logs))
            },
            |this, result, _cx| match result {
                Ok((result, accounts, logs)) => {
                    let summary = result
                        .get("summary")
                        .and_then(|value| value.as_str())
                        .unwrap_or("签到完成")
                        .to_string();
                    this.last_signin = Some(summary.clone());
                    this.apply_accounts(accounts);
                    this.logs = logs.logs;
                    this.connection = Connection::Connected;
                    this.status_error = false;
                    this.status = format!("签到完成：{summary}");
                }
                Err(error) => this.fail(error),
            },
        );
    }

    pub fn add_account(&mut self, cx: &mut Context<Self>, req: AddAccountRequest) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在添加账号",
            async move {
                client.add_account(&req).await?;
                client.list_accounts().await
            },
            |this, result, _cx| match result {
                Ok(accounts) => {
                    this.apply_accounts(accounts);
                    this.connection = Connection::Connected;
                    this.status_error = false;
                    this.status = "账号已添加".to_string();
                }
                Err(error) => this.fail(error),
            },
        );
    }

    pub fn delete_account(&mut self, cx: &mut Context<Self>, id: String) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在删除账号",
            async move {
                client.delete_account(&id).await?;
                client.list_accounts().await
            },
            |this, result, _cx| match result {
                Ok(accounts) => {
                    this.apply_accounts(accounts);
                    this.status_error = false;
                    this.status = "账号已删除".to_string();
                }
                Err(error) => this.fail(error),
            },
        );
    }

    pub fn set_schedule(&mut self, cx: &mut Context<Self>, id: String, time: Option<String>) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在保存签到时间",
            async move {
                client.set_schedule(&id, time.as_deref()).await?;
                client.list_accounts().await
            },
            |this, result, _cx| match result {
                Ok(accounts) => {
                    this.apply_accounts(accounts);
                    this.status_error = false;
                    this.status = "签到时间已保存".to_string();
                }
                Err(error) => this.fail(error),
            },
        );
    }

    pub fn send_code(&mut self, cx: &mut Context<Self>, phone: String) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在下发验证码",
            async move { client.send_code(&phone).await },
            |this, result, _cx| match result {
                Ok(value) => {
                    let message = value
                        .get("message")
                        .and_then(|value| value.as_str())
                        .unwrap_or("验证码已发送")
                        .to_string();
                    this.status_error = false;
                    this.status = message;
                }
                Err(error) => this.fail(error),
            },
        );
    }

    pub fn update_config(&mut self, cx: &mut Context<Self>, patch: Value) {
        let client = self.client.clone();
        self.execute(
            cx,
            "正在保存配置",
            async move {
                client.update_config(&patch).await?;
                client.config().await
            },
            |this, result, _cx| match result {
                Ok(config) => {
                    this.config = Some(config);
                    this.status_error = false;
                    this.status = "配置已保存".to_string();
                }
                Err(error) => this.fail(error),
            },
        );
    }

    /// 修改服务地址后重建客户端并重新探活（设置页用）。
    pub fn reconnect(&mut self, cx: &mut Context<Self>, new_base: String) -> Result<(), String> {
        let client = ApiClient::new(new_base.clone()).map_err(|error| error.to_string())?;
        self.client = client;
        self.base_url = new_base;
        self.bootstrap(cx);
        Ok(())
    }

    pub fn set_refresh_secs(&mut self, secs: u64, cx: &mut Context<Self>) {
        self.refresh_secs = secs.clamp(5, 3600);
        self.status = format!("自动刷新间隔已设为 {} 秒", self.refresh_secs);
        self.status_error = false;
        cx.notify();
    }

    /// 后台轮询：间隔读自 `refresh_secs`（设置页可改），仅在已连接且不忙时刷新。
    pub fn start_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let secs = this
                    .read_with(&*cx, |state, _| state.refresh_secs)
                    .unwrap_or(30)
                    .clamp(5, 3600);
                cx.background_executor()
                    .timer(Duration::from_secs(secs))
                    .await;
                let _ = this.update(cx, |state, cx| {
                    if state.connection == Connection::Connected && !state.busy {
                        state.refresh(cx);
                    }
                });
            }
        })
        .detach();
    }

    fn apply_accounts(&mut self, response: AccountsResponse) {
        self.accounts = response.accounts;
        self.today = response.today;
    }

    fn fail(&mut self, error: ApiError) {
        let message = error.to_string();
        self.connection = Connection::Failed(message.clone());
        self.status = format!("操作失败：{message}");
        self.status_error = true;
        self.busy = false;
    }
}
