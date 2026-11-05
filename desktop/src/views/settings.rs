//! 设置页：连接与安全边界说明、服务地址、自动刷新间隔、全局配置。
//!
//! 桌面形态免鉴权，本页不提供任何「修改口令」功能。

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};
use gpui_component::{
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement as _,
};
use serde_json::json;

use crate::state::AppState;

use super::{ACCENT, ACCENT_SOFT, MUTED, TEXT, action_button, card};

pub struct SettingsView {
    state: Entity<AppState>,
    base_input: Entity<InputState>,
    interval_input: Entity<InputState>,
    schedule_input: Entity<InputState>,
}

impl SettingsView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let base = state.read(cx).base_url.clone();
        let refresh_secs = state.read(cx).refresh_secs.to_string();
        let default_schedule = state
            .read(cx)
            .config
            .as_ref()
            .map(|config| config.default_schedule.clone())
            .unwrap_or_default();
        let base_input = cx.new(|cx| {
            InputState::new(&mut *window, cx)
                .placeholder("http://127.0.0.1:8787")
                .default_value(base)
        });
        let interval_input = cx.new(|cx| {
            InputState::new(&mut *window, cx)
                .placeholder("秒（5-3600）")
                .default_value(refresh_secs)
        });
        let schedule_input = cx.new(|cx| {
            InputState::new(&mut *window, cx)
                .placeholder("HH:MM")
                .default_value(default_schedule)
        });
        Self {
            state,
            base_input,
            interval_input,
            schedule_input,
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let base = state.base_url.clone();
        let refresh_secs = state.refresh_secs;
        let config = state.config.clone();
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
        let config_lines = match config {
            Some(config) => vec![
                format!("默认签到时间：{}", config.default_schedule),
                format!("金币任务：{}", on_off(config.coin_tasks)),
                format!("云游戏时长：{}", on_off(config.cloud_duration)),
                format!("分享平台：{}", config.share_platform),
                format!("加密策略：{}", config.crypto_policy),
                format!("内网白名单：{}", config.lan_cidrs),
                format!("内网免鉴权：{}", on_off(config.lan_no_auth)),
            ],
            None => vec!["尚未读取到配置（连接后自动加载）。".to_string()],
        };
        let mut config_elements: Vec<gpui::AnyElement> = Vec::new();
        for line in config_lines {
            config_elements.push(
                div()
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child(line)
                    .into_any_element(),
            );
        }

        div()
            .size_full()
            .flex_col()
            .gap(px(12.))
            .child(
                card()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT))
                            .child("连接与安全边界"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(format!("当前服务地址：{base}")),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(format!("服务端形态：{edition} · 鉴权模式：{auth_mode}")),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(
                                "Windows 桌面形态编译期强制免鉴权，安全边界是「仅监听 127.0.0.1 + 回环来源校验」，不适用于远程或 LAN 访问；远程管理请使用 Linux / Docker 的 webui 形态。",
                            ),
                    ),
            )
            .child(
                card()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT))
                            .child("服务地址"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("修改后立即重新探测 /api/meta；也可用环境变量 TAYGEDO_API_BASE 覆盖默认值。"),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(div().w(px(280.)).child(Input::new(&self.base_input)))
                            .child(
                                action_button("应用并重连", ACCENT, 0xffffff).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let new_base = this
                                            .base_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        if new_base.is_empty() {
                                            this.state.update(cx, |state, cx| {
                                                state.status = "服务地址不能为空".to_string();
                                                state.status_error = true;
                                                cx.notify();
                                            });
                                            return;
                                        }
                                        let result = this
                                            .state
                                            .update(cx, |state, cx| state.reconnect(cx, new_base));
                                        if let Err(error) = result {
                                            this.state.update(cx, |state, cx| {
                                                state.status = format!("服务地址无效：{error}");
                                                state.status_error = true;
                                                cx.notify();
                                            });
                                        }
                                    }),
                                ),
                            ),
                    ),
            )
            .child(
                card()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT))
                            .child(format!("自动刷新间隔（当前 {refresh_secs} 秒）")),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(div().w(px(160.)).child(Input::new(&self.interval_input)))
                            .child(
                                action_button("应用间隔", ACCENT_SOFT, ACCENT).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let value = this
                                            .interval_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        match value.parse::<u64>() {
                                            Ok(secs) => {
                                                this.state.update(cx, |state, cx| {
                                                    state.set_refresh_secs(secs, cx)
                                                });
                                            }
                                            Err(_) => {
                                                this.state.update(cx, |state, cx| {
                                                    state.status =
                                                        "刷新间隔须为 5-3600 之间的整数".to_string();
                                                    state.status_error = true;
                                                    cx.notify();
                                                });
                                            }
                                        }
                                    }),
                                ),
                            ),
                    ),
            )
            .child(
                card()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT))
                            .child("全局配置（服务端）"),
                    )
                    .child(
                        div()
                            .flex_col()
                            .gap(px(2.))
                            .children(config_elements),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(div().w(px(160.)).child(Input::new(&self.schedule_input)))
                            .child(
                                action_button("保存默认签到时间", ACCENT, 0xffffff).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        let value = this
                                            .schedule_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        if value.is_empty() {
                                            this.state.update(cx, |state, cx| {
                                                state.status = "默认签到时间不能为空".to_string();
                                                state.status_error = true;
                                                cx.notify();
                                            });
                                            return;
                                        }
                                        let patch = json!({ "default_schedule": value });
                                        this.state.update(cx, |state, cx| {
                                            state.update_config(cx, patch)
                                        });
                                    }),
                                ),
                            ),
                    ),
            )
            .overflow_y_scrollbar()
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "开启" } else { "关闭" }
}
