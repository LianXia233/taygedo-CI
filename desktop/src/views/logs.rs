//! 运行日志面板：标题栏（刷新）、账号筛选标签、深色日志区。对齐 WebUI 的 `.panel`。

use gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    ScrollHandle, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px,
    rgb,
};
use gpui_component::{h_flex, v_flex};

use crate::client::{AccountRow, LogRow};
use crate::theme::{Palette, RADIUS_CARD};

use super::{ButtonKind, RootView, button};

/// 日志面板需要的全部数据。
pub struct Panel<'a> {
    pub accounts: &'a [AccountRow],
    /// 已按当前筛选过滤过的日志。
    pub logs: &'a [LogRow],
    /// `None` 表示「全部」。
    pub filter: Option<&'a str>,
    pub palette: &'static Palette,
    pub scroll: &'a ScrollHandle,
}

pub fn panel(data: Panel<'_>, cx: &mut Context<RootView>) -> AnyElement {
    let p = data.palette;

    let mut tabs: Vec<AnyElement> = Vec::new();
    tabs.push(
        tab(
            "log-tab-all",
            "全部",
            data.filter.is_none(),
            p,
            cx.listener(|this, _, _, cx| {
                this.state
                    .update(cx, |state, cx| state.select_filter(cx, None));
            }),
        )
        .into_any_element(),
    );
    for account in data.accounts {
        let id = account.id.clone();
        let tab_id = account.id.as_str();
        tabs.push(
            tab(
                format!("log-tab-{tab_id}"),
                display_name(account),
                data.filter == Some(account.id.as_str()),
                p,
                cx.listener(move |this, _, _, cx| {
                    let id = id.clone();
                    this.state
                        .update(cx, |state, cx| state.select_filter(cx, Some(id)));
                }),
            )
            .into_any_element(),
        );
    }

    let mut lines: Vec<AnyElement> = Vec::new();
    if data.logs.is_empty() {
        lines.push(
            div()
                .w_full()
                .py(px(20.))
                .text_size(px(12.))
                .text_color(rgb(p.log_ts))
                .child(if data.filter.is_none() {
                    "暂无日志"
                } else {
                    "该账号暂无日志"
                })
                .into_any_element(),
        );
    } else {
        for log in data.logs {
            lines.push(line(log, p));
        }
    }

    v_flex()
        .w_full()
        .rounded(px(RADIUS_CARD))
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.outline))
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px(px(20.))
                .py(px(16.))
                .border_b_1()
                .border_color(rgb(p.outline))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(p.text))
                        .child("运行日志"),
                )
                .child(
                    button("btn-refresh-logs", "刷新", ButtonKind::Surface, p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.refresh(cx));
                        }),
                    ),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .flex_wrap()
                .gap(px(6.))
                .px(px(16.))
                .py(px(10.))
                .bg(rgb(p.surface_sunken))
                .border_b_1()
                .border_color(rgb(p.outline))
                .children(tabs),
        )
        .child(
            div()
                .id("log-body")
                .w_full()
                .h(px(400.))
                .overflow_y_scroll()
                .track_scroll(data.scroll)
                .bg(rgb(p.log_bg))
                .px(px(18.))
                .py(px(14.))
                .child(v_flex().w_full().gap(px(2.)).children(lines)),
        )
        .into_any_element()
}

fn display_name(account: &AccountRow) -> SharedString {
    let role = account.role_name.as_deref().unwrap_or("").trim();
    if !role.is_empty() {
        return role.to_string().into();
    }
    let name = account.name.trim();
    if name.is_empty() {
        "未命名账号".into()
    } else {
        name.to_string().into()
    }
}

/// 单个筛选标签：选中时主色底白字，未选中时透明底次要色。
fn tab(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    p: &'static Palette,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let (background, foreground) = if active {
        (p.primary, p.on_primary)
    } else {
        (p.surface_sunken, p.text_secondary)
    };
    h_flex()
        .id(id)
        .items_center()
        .px(px(14.))
        .py(px(5.))
        .rounded(px(999.))
        .bg(rgb(background))
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(foreground))
        .whitespace_nowrap()
        .child(label.into())
        .on_click(on_click)
}

/// 一行日志：时间戳（灰）、级别（定宽着色）、消息。
fn line(log: &LogRow, p: &'static Palette) -> AnyElement {
    let level = log.level.to_lowercase();
    let color = match level.as_str() {
        "warn" | "warning" => p.log_warn,
        "error" | "err" => p.log_err,
        "info" => p.log_info,
        _ => p.log_text,
    };
    let label = if log.level.is_empty() {
        "INFO".to_string()
    } else {
        log.level.to_uppercase()
    };
    h_flex()
        .w_full()
        .items_start()
        .gap(px(8.))
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(rgb(p.log_ts))
                .child(log.ts.clone()),
        )
        .child(
            div()
                .w(px(46.))
                .flex_none()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(color))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(rgb(p.log_text))
                .child(log.message.clone()),
        )
        .into_any_element()
}
