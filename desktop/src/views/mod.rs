//! 视图层根视图：页头、统计行、账号网格、日志面板、弹窗层、Toast 与连接失败提示。
//!
//! 结构逐条对齐 WebUI 的 `src/ui.html`：
//! `header` / `.stats` / `.main`（`.accounts` + `.panel`）/ `.modal-mask` / `.toast`。
//!
//! 这一层只做两件事：把状态层（`crate::state`）里的数据画出来，以及把输入框里
//! 的值读出来交给状态层。所有网络动作、弹窗流转、Toast 生命周期都在状态层。

pub mod add_account;
pub mod cards;
pub mod logs;
pub mod settings;

use std::collections::HashMap;

use gpui::{
    AnyElement, AppContext as _, Context, Div, ElementId, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, rgb,
    rgba,
};
use gpui_component::{
    Theme as ComponentTheme, ThemeMode, h_flex,
    input::{InputEvent, InputState},
    v_flex,
};

use crate::client::{AccountRow, LogRow};
use crate::state::{AppState, Connection, LoginMode, Modal, Toast, ToastKind, config_or_default};
use crate::theme::{Palette, RADIUS_CARD, RADIUS_MD, RADIUS_MODAL};

/// 按钮语义，对应 WebUI 的 `.btn` / `.btn.primary` / `.btn.danger`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// 白底描边（默认按钮）。
    Surface,
    /// 主色渐变（主操作）。
    Primary,
    /// 危险色文字 + 危险色描边（删除）。
    Danger,
}

/// 一个账号的签到时间输入框，外加它订阅的「按回车提交」回调。
struct ScheduleField {
    input: Entity<InputState>,
    /// 上一次从服务端同步进来的值：只有在服务端值变化时才回填，避免把用户正在
    /// 输入的内容每帧覆盖掉（WebUI 是整块 innerHTML 重建，没有这个问题）。
    synced: String,
    /// 订阅句柄必须持有，drop 即退订。
    _enter: Subscription,
}

/// 根视图：持有窗口级共享的输入框与临时表单状态。
pub struct RootView {
    state: Entity<AppState>,

    // ---- 添加账号弹窗 ----
    phone: Entity<InputState>,
    password: Entity<InputState>,
    captcha: Entity<InputState>,
    name: Entity<InputState>,
    mode: LoginMode,

    // ---- 全局设置弹窗 ----
    cfg_schedule: Entity<InputState>,
    cfg_share: Entity<InputState>,
    cfg_coin: bool,
    cfg_cloud: bool,

    // ---- 账号卡片的签到时间输入框（按账号 id 惰性创建）----
    schedules: HashMap<String, ScheduleField>,

    /// 上一次渲染时的弹窗，用于识别「刚关闭」以清空表单。
    last_modal: Modal,
    log_scroll: ScrollHandle,
    /// 日志指纹（条数 + 最后一条内容）：变化即视为「有新日志」，把滚动条贴底。
    log_key: String,
}

impl RootView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let phone = cx.new(|cx| InputState::new(&mut *window, cx).placeholder("请输入手机号"));
        let password = cx.new(|cx| {
            InputState::new(&mut *window, cx)
                .masked(true)
                .placeholder("请输入密码")
        });
        let captcha = cx.new(|cx| InputState::new(&mut *window, cx).placeholder("短信验证码"));
        let name = cx.new(|cx| InputState::new(&mut *window, cx).placeholder("如：主账号"));
        let cfg_schedule = cx.new(|cx| InputState::new(&mut *window, cx).placeholder("HH:MM"));
        let cfg_share =
            cx.new(|cx| InputState::new(&mut *window, cx).placeholder("qq / wechat / weibo"));

        let mut view = Self {
            state,
            phone,
            password,
            captcha,
            name,
            mode: LoginMode::Password,
            cfg_schedule,
            cfg_share,
            cfg_coin: true,
            cfg_cloud: true,
            schedules: HashMap::new(),
            last_modal: Modal::None,
            log_scroll: ScrollHandle::new(),
            log_key: String::new(),
        };
        view.sync_modal(window, cx);

        // AppState 的任何变化都驱动整窗重绘；顺带检测弹窗开合以同步表单。
        let state = view.state.clone();
        cx.observe_in(&state, window, |this, _state, window, cx| {
            this.sync_modal(window, cx);
            cx.notify();
        })
        .detach();

        view
    }

    // ---- 表单同步 ----

    /// 弹窗切换时的表单初始化，对应 WebUI 的 `addBtn` / `openSettings` / 提交成功后的清空。
    fn sync_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let modal = self.state.read(cx).modal.clone();
        if modal == self.last_modal {
            return;
        }
        if matches!(self.last_modal, Modal::AddAccount) {
            self.clear_add_form(window, cx);
        }
        match &modal {
            Modal::AddAccount => self.mode = LoginMode::Password,
            Modal::Settings => self.sync_settings_form(window, cx),
            Modal::None | Modal::ConfirmDelete { .. } => {}
        }
        self.last_modal = modal;
    }

    fn clear_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.phone, &self.password, &self.captcha, &self.name] {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.mode = LoginMode::Password;
    }

    fn sync_settings_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = {
            let state = self.state.read(cx);
            config_or_default(&state.config)
        };
        self.cfg_schedule.update(cx, |input, cx| {
            input.set_value(config.default_schedule.clone(), window, cx);
        });
        self.cfg_share.update(cx, |input, cx| {
            input.set_value(config.share_platform.clone(), window, cx);
        });
        self.cfg_coin = config.coin_tasks;
        self.cfg_cloud = config.cloud_duration;
    }

    fn set_mode(&mut self, mode: LoginMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    fn set_coin(&mut self, value: bool, cx: &mut Context<Self>) {
        self.cfg_coin = value;
        cx.notify();
    }

    fn set_cloud(&mut self, value: bool, cx: &mut Context<Self>) {
        self.cfg_cloud = value;
        cx.notify();
    }

    /// 保证每个账号都有一个签到时间输入框，并在服务端值变化时回填。
    fn ensure_schedules(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accounts = self.state.read(cx).accounts.clone();
        self.schedules
            .retain(|id, _| accounts.iter().any(|row| &row.id == id));
        for row in &accounts {
            if let Some(field) = self.schedules.get_mut(&row.id) {
                if field.synced != row.schedule {
                    field.synced = row.schedule.clone();
                    let value = row.schedule.clone();
                    field
                        .input
                        .update(cx, |input, cx| input.set_value(value, window, cx));
                }
                continue;
            }
            let input = cx.new(|cx| {
                InputState::new(&mut *window, cx)
                    .placeholder("HH:MM")
                    .default_value(row.schedule.clone())
            });
            let account_id = row.id.clone();
            let handle = input.clone();
            let enter = cx.subscribe_in(
                &input,
                window,
                move |this, _input, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let value = handle.read(cx).value().to_string();
                        let id = account_id.clone();
                        this.state
                            .update(cx, |state, cx| state.set_schedule(cx, id, value));
                    }
                },
            );
            self.schedules.insert(
                row.id.clone(),
                ScheduleField {
                    input,
                    synced: row.schedule.clone(),
                    _enter: enter,
                },
            );
        }
    }

    // ---- 渲染片段 ----

    fn header(&self, p: &'static Palette, cx: &Context<Self>) -> Div {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap(px(14.))
            .px(px(24.))
            .py(px(16.))
            .rounded(px(RADIUS_CARD))
            .bg(rgb(p.surface))
            .border_1()
            .border_color(rgb(p.outline))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(14.))
                    .child(
                        div()
                            .w(px(44.))
                            .h(px(44.))
                            .flex_none()
                            .rounded(px(RADIUS_MD))
                            .bg(rgb(p.primary))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(19.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(p.on_primary))
                            .child("签"),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .whitespace_nowrap()
                                    .child("塔吉多自动签到"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.text_secondary))
                                    .child("多账号 · 每日定时 · 云游戏时长"),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        button("btn-theme", "主题", ButtonKind::Surface, p).on_click(cx.listener(
                            |this, _, window, cx| {
                                this.state.update(cx, |state, cx| state.toggle_theme(cx));
                                // 自绘部分走 `Palette`，但输入框 / 开关这类组件是
                                // gpui-component 的主题驱动的，要一起切才不会半深半浅。
                                let dark = { this.state.read(cx).dark };
                                let mode = if dark {
                                    ThemeMode::Dark
                                } else {
                                    ThemeMode::Light
                                };
                                ComponentTheme::change(mode, Some(window), cx);
                            },
                        )),
                    )
                    .child(
                        button("btn-settings", "设置", ButtonKind::Surface, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.open_settings(cx));
                            }),
                        ),
                    )
                    .child(
                        button("btn-add", "添加账号", ButtonKind::Primary, p).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.open_add(cx));
                            }),
                        ),
                    ),
            )
    }

    fn stats_row(stats: (usize, usize, usize), p: &'static Palette) -> Div {
        let (total, done, pending) = stats;
        div()
            .w_full()
            .grid()
            .grid_cols(3)
            .gap(px(14.))
            .child(stat_card(total, "总账号", p.primary, p))
            .child(stat_card(done, "今日已签到", p.success, p))
            .child(stat_card(pending, "待签到", p.warning, p))
    }

    fn snapshot(&self, cx: &Context<Self>) -> Snapshot {
        let state = self.state.read(cx);
        Snapshot {
            palette: state.palette(),
            stats: state.stats(),
            accounts: state.accounts.clone(),
            logs: state.visible_logs().into_iter().cloned().collect(),
            log_filter: state.log_filter.clone(),
            modal: state.modal.clone(),
            toast: state.toast.clone(),
            connection: state.connection.clone(),
            code_countdown: state.code_countdown,
            base_url: state.base_url.clone(),
            busy: state.busy,
        }
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_schedules(window, cx);
        let snap = self.snapshot(cx);
        let p = snap.palette;

        // 有新日志就贴底，对齐 WebUI 的 `box.scrollTop = box.scrollHeight`。
        let log_key = log_fingerprint(&snap.logs);
        if log_key != self.log_key {
            self.log_key = log_key;
            self.log_scroll.scroll_to_bottom();
        }

        // ---- 账号网格 ----
        let mut grid: Vec<AnyElement> = Vec::new();
        if snap.accounts.is_empty() {
            grid.push(cards::empty_state(p));
        } else {
            for row in &snap.accounts {
                let Some(field) = self.schedules.get(&row.id) else {
                    continue;
                };
                let signing = self.state.read(cx).signing(&row.id);
                grid.push(
                    cards::card(
                        cards::Card {
                            row,
                            signing,
                            schedule: &field.input,
                            palette: p,
                        },
                        cx,
                    )
                    .into_any_element(),
                );
            }
        }

        // ---- 日志面板 ----
        let log_panel = logs::panel(
            logs::Panel {
                accounts: &snap.accounts,
                logs: &snap.logs,
                filter: snap.log_filter.as_deref(),
                palette: p,
                scroll: &self.log_scroll,
            },
            cx,
        );

        // ---- 连接失败提示 ----
        let banner = match &snap.connection {
            Connection::Failed(message) => Some(banner(message, &snap.base_url, p, cx)),
            Connection::Connected => None,
        };

        // ---- 弹窗层 ----
        let modal = match &snap.modal {
            Modal::AddAccount => Some(add_account::modal(
                add_account::Form {
                    phone: &self.phone,
                    password: &self.password,
                    captcha: &self.captcha,
                    name: &self.name,
                    mode: self.mode,
                    countdown: snap.code_countdown,
                    busy: snap.busy,
                },
                p,
                cx,
            )),
            Modal::Settings => Some(settings::modal(
                settings::Form {
                    schedule: &self.cfg_schedule,
                    share: &self.cfg_share,
                    coin: self.cfg_coin,
                    cloud: self.cfg_cloud,
                },
                p,
                cx,
            )),
            Modal::ConfirmDelete { id, label } => Some(confirm_delete(id, label, p, cx)),
            Modal::None => None,
        };

        let toast = snap.toast.as_ref().map(|toast| toast_layer(toast, p));

        v_flex()
            .size_full()
            .relative()
            .bg(rgb(p.bg))
            .child(
                v_flex()
                    .id("root-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .w_full()
                            .px(px(20.))
                            .py(px(24.))
                            .gap(px(20.))
                            .child(self.header(p, cx))
                            .child(Self::stats_row(snap.stats, p))
                            .children(banner)
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_start()
                                    .gap(px(20.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .grid()
                                            .grid_cols(2)
                                            .gap(px(16.))
                                            .children(grid),
                                    )
                                    .child(div().w(px(430.)).flex_none().child(log_panel)),
                            ),
                    ),
            )
            .children(modal)
            .children(toast)
    }
}

/// 一次渲染用到的状态快照。
///
/// 状态层是 `Entity<AppState>`，渲染期间若一直持有 `read()` 的借用就没法创建
/// 点击回调（需要 `&mut Context`），所以先拷一份再画。
struct Snapshot {
    palette: &'static Palette,
    stats: (usize, usize, usize),
    accounts: Vec<AccountRow>,
    logs: Vec<LogRow>,
    log_filter: Option<String>,
    modal: Modal,
    toast: Option<Toast>,
    connection: Connection,
    code_countdown: u64,
    base_url: String,
    busy: bool,
}

/// 通用按钮底样（对应 WebUI 的 `.btn`）。
///
/// gpui-fast 的 fluent `on_click` 只在 `StatefulInteractiveElement` 上，因此这里
/// 先 `.id(...)` 把 `Div` 变成 `Stateful<Div>`，调用方才能链式接 `.on_click(...)`。
/// `id` 必须跨帧稳定且同帧唯一：单实例按钮用静态字符串，列表内用行标识拼接。
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    p: &Palette,
) -> Stateful<Div> {
    let (background, foreground, border) = match kind {
        ButtonKind::Surface => (p.surface, p.text, p.outline),
        ButtonKind::Primary => (p.primary, p.on_primary, p.primary),
        ButtonKind::Danger => (p.surface, p.danger, p.danger),
    };
    h_flex()
        .id(id)
        .items_center()
        .justify_center()
        .gap(px(7.))
        .min_h(px(40.))
        .px(px(16.))
        .py(px(8.))
        .rounded(px(999.))
        .bg(rgb(background))
        .border_1()
        .border_color(rgb(border))
        .text_size(px(13.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(foreground))
        .whitespace_nowrap()
        .child(label.into())
}

/// 表单里的一「栏」：小标题 + 控件。
pub fn field(label: impl Into<SharedString>, control: impl IntoElement, p: &Palette) -> AnyElement {
    v_flex()
        .w_full()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.text_secondary))
                .child(label.into()),
        )
        .child(control)
        .into_any_element()
}

/// 弹窗外壳：半透明遮罩 + 460 宽圆角卡片 + 标题栏（含关闭按钮）。
pub fn modal_shell(
    title: impl Into<SharedString>,
    body: impl IntoElement,
    p: &'static Palette,
    cx: &mut Context<RootView>,
) -> AnyElement {
    let title: SharedString = title.into();
    div()
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .right(px(0.))
        .bottom(px(0.))
        .flex()
        .items_center()
        .justify_center()
        .p(px(16.))
        .bg(rgba(0x0f_17_2a_73))
        // 遮罩自己吃掉点击，避免点穿到底下的卡片与按钮。
        .id("modal-mask")
        .on_click(|_, _, _| {})
        .child(
            v_flex()
                .id("modal-card")
                .w(px(460.))
                .max_h(px(760.))
                .overflow_y_scroll()
                .rounded(px(RADIUS_MODAL))
                .bg(rgb(p.surface))
                .border_1()
                .border_color(rgb(p.outline))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .px(px(24.))
                        .py(px(18.))
                        .border_b_1()
                        .border_color(rgb(p.outline))
                        .child(
                            div()
                                .text_size(px(17.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(p.text))
                                .child(title),
                        )
                        .child(
                            h_flex()
                                .id("modal-close")
                                .w(px(32.))
                                .h(px(32.))
                                .items_center()
                                .justify_center()
                                .rounded(px(999.))
                                .text_size(px(20.))
                                .text_color(rgb(p.text_tertiary))
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |state, cx| state.close_modal(cx));
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .w_full()
                        .gap(px(14.))
                        .px(px(24.))
                        .pt(px(20.))
                        .pb(px(28.))
                        .child(body),
                ),
        )
        .into_any_element()
}

/// 删除确认弹窗：WebUI 用原生 `confirm()`，桌面端用等价的自绘弹窗。
fn confirm_delete(
    id: &str,
    label: &str,
    p: &'static Palette,
    cx: &mut Context<RootView>,
) -> AnyElement {
    let account_id = id.to_string();
    let body = v_flex()
        .w_full()
        .gap(px(16.))
        .child(
            v_flex()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(rgb(p.text))
                        .child("确认删除该账号？删除后需重新登录。"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(p.text_secondary))
                        .child(label.to_string()),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .justify_end()
                .gap(px(10.))
                .child(
                    button("btn-cancel-delete", "取消", ButtonKind::Surface, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.close_modal(cx));
                        }),
                    ),
                )
                .child(
                    button("btn-confirm-delete", "删除", ButtonKind::Danger, p).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let id = account_id.clone();
                            this.state
                                .update(cx, |state, cx| state.delete_account(cx, id));
                        }),
                    ),
                ),
        );
    modal_shell("删除账号", body, p, cx)
}

/// 账号卡片里的「今日已签 / 待签到」徽章。
fn stat_card(value: usize, label: &'static str, color: u32, p: &'static Palette) -> Div {
    v_flex()
        .flex_1()
        .gap(px(4.))
        .px(px(20.))
        .py(px(18.))
        .rounded(px(RADIUS_CARD))
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.outline))
        .child(
            div()
                .text_size(px(26.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(color))
                .child(value.to_string()),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(rgb(p.text_secondary))
                .child(label),
        )
}

fn banner(
    message: &str,
    base_url: &str,
    p: &'static Palette,
    cx: &mut Context<RootView>,
) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .px(px(16.))
        .py(px(12.))
        .rounded(px(RADIUS_MD))
        .bg(rgb(p.danger_soft))
        .border_1()
        .border_color(rgb(p.danger))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(p.danger))
                        .child(message.to_string()),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(p.text_secondary))
                        .child(format!("回环服务：{base_url}")),
                ),
        )
        .child(
            button("btn-retry", "重试", ButtonKind::Surface, p).on_click(cx.listener(
                |this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.refresh(cx));
                },
            )),
        )
        .into_any_element()
}

/// 底部居中浮条。出现与消失都由状态层的定时器负责，这里只负责画。
fn toast_layer(toast: &Toast, p: &'static Palette) -> AnyElement {
    let (background, foreground) = match toast.kind {
        ToastKind::Ok => (p.success, 0xffffff),
        ToastKind::Err => (p.danger, 0xffffff),
    };
    div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .bottom(px(28.))
        .flex()
        .justify_center()
        .child(
            div()
                .px(px(24.))
                .py(px(12.))
                .rounded(px(999.))
                .bg(rgb(background))
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(foreground))
                .child(toast.text.clone()),
        )
        .into_any_element()
}

/// 供子视图构造提交载荷：把表单里的文本读出来。
pub fn read_input(input: &Entity<InputState>, cx: &Context<RootView>) -> String {
    input.read(cx).value().to_string()
}

/// 日志列表的指纹：条数 + 最后一条的时间戳与正文。
///
/// 只看条数不够——日志上限 200 条后新增不会改变条数，但内容会变。
fn log_fingerprint(logs: &[LogRow]) -> String {
    let count = logs.len();
    match logs.last() {
        Some(last) => {
            let (ts, message) = (last.ts.as_str(), last.message.as_str());
            format!("{count}|{ts}|{message}")
        }
        None => format!("{count}"),
    }
}
