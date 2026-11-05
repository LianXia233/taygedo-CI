//! 账号卡片与空状态：对齐 WebUI 的 `renderAccounts`。

use gpui::{
    AnyElement, Context, Entity, FontWeight, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px, rgb,
};
use gpui_component::{
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use crate::client::AccountRow;
use crate::theme::{Palette, RADIUS_CARD, RADIUS_MD};

use super::{ButtonKind, RootView, button};

/// 单张账号卡片需要的全部数据。
pub struct Card<'a> {
    pub row: &'a AccountRow,
    /// 该账号正在签到（只禁用这一行的按钮，与 WebUI 的单按钮 loading 一致）。
    pub signing: bool,
    /// 该账号的签到时间输入框。
    pub schedule: &'a Entity<InputState>,
    pub palette: &'static Palette,
}

/// 无账号时的空状态占位。
pub fn empty_state(p: &'static Palette) -> AnyElement {
    v_flex()
        .col_span_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .p(px(48.))
        .rounded(px(RADIUS_CARD))
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.outline))
        .child(
            div()
                .text_size(px(15.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(p.text))
                .child("还没有账号"),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(rgb(p.text_secondary))
                .child("点击右上角「添加账号」开始使用"),
        )
        .into_any_element()
}

/// 单张账号卡片。
pub fn card(data: Card<'_>, cx: &mut Context<RootView>) -> AnyElement {
    let p = data.palette;
    let row = data.row;
    let title = display_name(row);
    let initial = title
        .chars()
        .next()
        .map(|c| c.to_string())
        .unwrap_or_default();
    let signing = data.signing;

    let mut tags: Vec<AnyElement> = Vec::new();
    if let Some(phone) = row.phone.as_deref().filter(|value| !value.is_empty()) {
        tags.push(tag(phone.to_string(), p));
    }
    let uid = if row.uid.is_empty() {
        "-".to_string()
    } else {
        row.uid.clone()
    };
    tags.push(tag(format!("UID {uid}"), p));

    let signin_id = row.id.clone();
    let delete_id = row.id.clone();
    let delete_label = title.to_string();
    let row_id = row.id.as_str();
    let signin_button = button(
        format!("signin-{row_id}"),
        if signing {
            "签到中…"
        } else {
            "立即签到"
        },
        ButtonKind::Primary,
        p,
    )
    .flex_1();
    let signin_button = if signing {
        signin_button.opacity(0.55)
    } else {
        signin_button.on_click(cx.listener(move |this, _, _, cx| {
            let id = signin_id.clone();
            this.state.update(cx, |state, cx| state.signin(cx, id));
        }))
    };

    v_flex()
        .w_full()
        .gap(px(16.))
        .p(px(20.))
        .rounded(px(RADIUS_CARD))
        .bg(rgb(p.surface))
        .border_1()
        .border_color(rgb(p.outline))
        .child(
            h_flex()
                .items_start()
                .gap(px(14.))
                .child(
                    div()
                        .w(px(50.))
                        .h(px(50.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(16.))
                        .bg(rgb(p.primary_soft))
                        .text_size(px(20.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(p.primary_ink))
                        .child(initial),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.))
                        .child(
                            h_flex()
                                .items_center()
                                .flex_wrap()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(p.text))
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(title),
                                )
                                .child(badge(row.signed_today, p)),
                        )
                        .child(h_flex().flex_wrap().gap(px(6.)).children(tags)),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(RADIUS_MD))
                .bg(rgb(p.surface_sunken))
                .border_1()
                .border_color(rgb(p.outline))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(p.text_secondary))
                        .child("每日签到时间"),
                )
                .child(Input::new(data.schedule).w(px(116.))),
        )
        .child(h_flex().w_full().gap(px(10.)).child(signin_button).child(
            button(format!("delete-{row_id}"), "删除", ButtonKind::Danger, p).on_click(
                cx.listener(move |this, _, _, cx| {
                    let id = delete_id.clone();
                    let label = delete_label.clone();
                    this.state
                        .update(cx, |state, cx| state.ask_delete(cx, id, label));
                }),
            ),
        ))
        .into_any_element()
}

/// 卡片标题：优先游戏内角色名，其次备注名。
fn display_name(row: &AccountRow) -> SharedString {
    let role = row.role_name.as_deref().unwrap_or("").trim();
    if !role.is_empty() {
        return role.to_string().into();
    }
    let name = row.name.trim();
    if name.is_empty() {
        "未命名账号".into()
    } else {
        name.to_string().into()
    }
}

fn badge(signed: bool, p: &'static Palette) -> AnyElement {
    let (background, foreground, text) = if signed {
        (p.success_soft, p.success_ink, "今日已签")
    } else {
        (p.warning_soft, p.warning_ink, "待签到")
    };
    div()
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded(px(999.))
        .bg(rgb(background))
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(foreground))
        .child(text)
        .into_any_element()
}

fn tag(text: String, p: &'static Palette) -> AnyElement {
    div()
        .px(px(9.))
        .py(px(3.))
        .rounded(px(999.))
        .bg(rgb(p.outline_variant))
        .text_size(px(11.5))
        .text_color(rgb(p.text_secondary))
        .whitespace_nowrap()
        .child(text)
        .into_any_element()
}
