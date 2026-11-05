//! 视图层汇总：根视图负责页面切换与状态栏，子视图承载具体业务。

pub mod connection;
pub mod dashboard;
pub mod settings;

use gpui::{
    AnyView, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, px, rgb,
};
use gpui_component::{h_flex, v_flex};

use crate::state::{AppState, Connection};

// 浅色主题配色。
pub const BG: u32 = 0xf1f5f9;
pub const SURFACE: u32 = 0xffffff;
pub const TEXT: u32 = 0x1e293b;
pub const MUTED: u32 = 0x64748b;
pub const LINE: u32 = 0xe2e8f0;
pub const ACCENT: u32 = 0x2563eb;
pub const ACCENT_SOFT: u32 = 0xdbeafe;
pub const DANGER: u32 = 0xdc2626;
pub const OK: u32 = 0x16a34a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Settings,
}

/// 状态栏渲染参数。集中成结构体，避免 `status_bar` 参数个数超标。
struct StatusBarInfo {
    status: String,
    status_error: bool,
    busy: bool,
    base: String,
    edition: String,
    auth_mode: String,
    today: String,
}

/// 窗口根视图：顶部导航、中间内容槽位、底部状态栏。
pub struct RootView {
    state: Entity<AppState>,
    page: Page,
    dashboard: AnyView,
    settings: AnyView,
    connection: AnyView,
}

impl RootView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // AppState 变化（连接态、数据、状态消息）驱动整窗重绘。
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let dashboard: AnyView = cx
            .new(|cx| dashboard::DashboardView::new(state.clone(), &mut *window, cx))
            .into();
        let settings: AnyView = cx
            .new(|cx| settings::SettingsView::new(state.clone(), &mut *window, cx))
            .into();
        let connection: AnyView = cx
            .new(|cx| connection::ConnectionView::new(state.clone(), cx))
            .into();
        Self {
            state,
            page: Page::Dashboard,
            dashboard,
            settings,
            connection,
        }
    }

    fn header(&self, cx: &Context<Self>) -> impl IntoElement {
        let dashboard_active = self.page == Page::Dashboard;
        let settings_active = self.page == Page::Settings;
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .px(px(20.))
            .py(px(12.))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .text_size(px(16.))
                    .text_color(rgb(TEXT))
                    .child("塔吉多签到 · 桌面客户端"),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(nav_button("主面板", dashboard_active).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.page = Page::Dashboard;
                            cx.notify();
                        },
                    )))
                    .child(nav_button("设置", settings_active).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.page = Page::Settings;
                            cx.notify();
                        },
                    ))),
            )
    }

    fn status_bar(&self, info: StatusBarInfo) -> impl IntoElement {
        let prefix = if info.busy { "处理中 · " } else { "" };
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .px(px(20.))
            .py(px(8.))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(if info.status_error {
                        rgb(DANGER)
                    } else {
                        rgb(MUTED)
                    })
                    .child(format!("{prefix}{}", info.status)),
            )
            .child(
                h_flex()
                    .gap(px(14.))
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(format!("服务：{}", info.base))
                    .child(format!("形态：{}", info.edition))
                    .child(format!("鉴权：{}", info.auth_mode))
                    .child(format!("日期：{}", info.today)),
            )
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let connection = state.connection.clone();
        let status = state.status.clone();
        let status_error = state.status_error;
        let busy = state.busy;
        let base = state.base_url.clone();
        let today = state.today.clone();
        let (edition, auth_mode) = match &state.meta {
            Some(meta) => (
                if meta.edition.is_empty() {
                    "未知".to_string()
                } else {
                    meta.edition.clone()
                },
                if meta.auth_mode.is_empty() {
                    "未知".to_string()
                } else {
                    meta.auth_mode.clone()
                },
            ),
            None => ("未知".to_string(), "未知".to_string()),
        };

        let info = StatusBarInfo {
            status,
            status_error,
            busy,
            base,
            edition,
            auth_mode,
            today,
        };

        let failed = matches!(connection, Connection::Failed(_));
        let content: AnyView = if failed {
            self.connection.clone()
        } else {
            match self.page {
                Page::Dashboard => self.dashboard.clone(),
                Page::Settings => self.settings.clone(),
            }
        };

        v_flex()
            .size_full()
            .bg(rgb(BG))
            .child(self.header(cx))
            .child(div().flex_1().min_h_0().p(px(16.)).child(content))
            .child(self.status_bar(info))
    }
}

#[track_caller]
fn nav_button(label: &'static str, active: bool) -> gpui::Stateful<gpui::Div> {
    let (background, foreground) = if active {
        (ACCENT, 0xffffff)
    } else {
        (ACCENT_SOFT, ACCENT)
    };
    action_button(label, background, foreground)
}

/// 通用按钮底样。
///
/// gpui-fast 的 fluent `on_click` 定义在 `StatefulInteractiveElement` 上，只有
/// `Stateful<Div>` 才有；裸 `Div` 只有命令式的 `&mut self -> ()` 版本。因此这里
/// 必须先 `.id(...)` 返回 `Stateful<Div>`，调用方才能链式接 `.on_click(...)`。
///
/// ElementId 取 `#[track_caller]` 捕获的调用点位置：它跨帧恒定，符合 GPUI
/// retained mode 对稳定 ID 的要求。这里刻意不用自增序号 —— 每次 render 都会
/// 调用本函数，自增会让 ID 每帧都变，跨帧的 interactivity 状态就无法保留。
#[track_caller]
pub fn action_button(
    label: impl Into<SharedString>,
    background: u32,
    foreground: u32,
) -> gpui::Stateful<gpui::Div> {
    let id = gpui::ElementId::from(std::panic::Location::caller());
    let label: SharedString = label.into();
    div()
        .px(px(12.))
        .py(px(6.))
        .rounded(px(6.))
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_size(px(13.))
        .child(label)
        .id(id)
}

/// 白色卡片容器。
pub fn card() -> gpui::Div {
    v_flex()
        .gap(px(10.))
        .p(px(16.))
        .rounded(px(10.))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(LINE))
}
