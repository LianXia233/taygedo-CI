//! 「全局设置」弹窗：默认签到时间、金币任务、云异环时长、分享平台。
//!
//! 对齐 WebUI 的 `#settingsModal`，但**不含**「传输加密 / 修改密码 / 退出登录」三块：
//! 桌面单包只连本机回环、没有登录态，那些表单项在这里没有意义。

use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px, rgb,
};
use gpui_component::{
    h_flex,
    input::{Input, InputState},
    switch::Switch,
    v_flex,
};

use crate::state::SettingsPayload;
use crate::theme::Palette;

use super::{ButtonKind, RootView, button, field, modal_shell, read_input};

/// 设置表单需要的状态（输入框与开关值由根视图持有）。
pub struct Form<'a> {
    pub schedule: &'a Entity<InputState>,
    pub share: &'a Entity<InputState>,
    pub coin: bool,
    pub cloud: bool,
}

pub fn modal(form: Form<'_>, p: &'static Palette, cx: &mut Context<RootView>) -> AnyElement {
    let schedule = form.schedule.clone();
    let share = form.share.clone();
    let body = v_flex()
        .w_full()
        .gap(px(14.))
        .child(field(
            "默认签到时间（每天）",
            Input::new(form.schedule).w_full(),
            p,
        ))
        .child(switch_row(
            "金币任务",
            Switch::new("cfg-coin")
                .checked(form.coin)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.set_coin(*checked, cx);
                })),
            p,
        ))
        .child(switch_row(
            "云异环时长",
            Switch::new("cfg-cloud")
                .checked(form.cloud)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    this.set_cloud(*checked, cx);
                })),
            p,
        ))
        .child(field("分享平台", Input::new(form.share).w_full(), p))
        .child(
            button("btn-save-settings", "保存设置", ButtonKind::Primary, p)
                .w_full()
                .min_h(px(44.))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let coin = this.cfg_coin;
                    let cloud = this.cfg_cloud;
                    let payload = SettingsPayload {
                        default_schedule: read_input(&schedule, cx),
                        coin_tasks: coin,
                        cloud_duration: cloud,
                        share_platform: read_input(&share, cx),
                    };
                    this.state
                        .update(cx, |state, cx| state.save_settings(cx, payload));
                })),
        );
    modal_shell("全局设置", body, p, cx)
}

fn switch_row(label: &'static str, control: Switch, p: &'static Palette) -> AnyElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(p.text))
                .child(label),
        )
        .child(control)
        .into_any_element()
}
