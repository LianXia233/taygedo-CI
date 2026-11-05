//! 「添加账号」弹窗：密码 / 验证码两种登录方式，对齐 WebUI 的 `#addModal`。

use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, InteractiveElement as _, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div, px, rgb,
};
use gpui_component::{
    h_flex,
    input::{Input, InputState},
    v_flex,
};

use crate::state::{AddPayload, LoginMode};
use crate::theme::{Palette, RADIUS_MD, RADIUS_SM};

use super::{ButtonKind, RootView, button, field, modal_shell, read_input};

/// 弹窗需要读取的表单状态（输入框本体由根视图持有）。
pub struct Form<'a> {
    pub phone: &'a Entity<InputState>,
    pub password: &'a Entity<InputState>,
    pub captcha: &'a Entity<InputState>,
    pub name: &'a Entity<InputState>,
    pub mode: LoginMode,
    /// 验证码倒计时秒数；>0 时「发送验证码」显示 `Ns` 并禁用。
    pub countdown: u64,
    /// 有网络命令在执行：登录按钮显示「登录中…」并禁用（对齐 WebUI 的 `btn.disabled`）。
    pub busy: bool,
}

pub fn modal(form: Form<'_>, p: &'static Palette, cx: &mut Context<RootView>) -> AnyElement {
    let captcha_mode = form.mode == LoginMode::Captcha;

    let mut body = v_flex()
        .w_full()
        .gap(px(14.))
        .child(
            h_flex()
                .w_full()
                .gap(px(4.))
                .p(px(4.))
                .rounded(px(RADIUS_MD))
                .bg(rgb(p.outline_variant))
                .child(
                    mode_tab(
                        "tab-password",
                        "密码登录",
                        !captcha_mode,
                        p,
                        cx.listener(|this, _, _, cx| this.set_mode(LoginMode::Password, cx)),
                    )
                    .flex_1(),
                )
                .child(
                    mode_tab(
                        "tab-captcha",
                        "验证码登录",
                        captcha_mode,
                        p,
                        cx.listener(|this, _, _, cx| this.set_mode(LoginMode::Captcha, cx)),
                    )
                    .flex_1(),
                ),
        )
        .child(field("手机号", Input::new(form.phone).w_full(), p));

    if captcha_mode {
        let phone = form.phone.clone();
        let countdown = form.countdown;
        let send = if countdown > 0 {
            button(
                "btn-send-code",
                format!("{countdown}s"),
                ButtonKind::Surface,
                p,
            )
            .opacity(0.5)
        } else {
            button("btn-send-code", "发送验证码", ButtonKind::Surface, p).on_click(cx.listener(
                move |this, _, _, cx| {
                    let value = read_input(&phone, cx);
                    this.state
                        .update(cx, |state, cx| state.send_code(cx, value));
                },
            ))
        };
        body = body.child(field(
            "验证码",
            h_flex()
                .w_full()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(form.captcha).w_full()),
                )
                .child(send),
            p,
        ));
    } else {
        body = body.child(field("密码", Input::new(form.password).w_full(), p));
    }

    body = body
        .child(field("备注名（可选）", Input::new(form.name).w_full(), p))
        .child(submit_button(&form, p, cx));

    modal_shell("添加账号", body, p, cx)
}

fn submit_button(
    form: &Form<'_>,
    p: &'static Palette,
    cx: &mut Context<RootView>,
) -> Stateful<Div> {
    let base = button(
        "btn-login",
        if form.busy { "登录中…" } else { "登 录" },
        ButtonKind::Primary,
        p,
    )
    .w_full()
    .min_h(px(46.))
    .text_size(px(15.));
    if form.busy {
        return base.opacity(0.55);
    }
    let mode = form.mode;
    let phone = form.phone.clone();
    let password = form.password.clone();
    let captcha = form.captcha.clone();
    let name = form.name.clone();
    base.on_click(cx.listener(move |this, _, _, cx| {
        let payload = AddPayload {
            phone: read_input(&phone, cx),
            mode,
            password: if mode == LoginMode::Password {
                read_input(&password, cx)
            } else {
                String::new()
            },
            captcha: if mode == LoginMode::Captcha {
                read_input(&captcha, cx)
            } else {
                String::new()
            },
            name: read_input(&name, cx),
        };
        this.state
            .update(cx, |state, cx| state.submit_add(cx, payload));
    }))
}

/// 分段选项卡：选中项白底主色字，未选中透明（对齐 WebUI 的 `.tab`）。
fn mode_tab(
    id: &'static str,
    label: &'static str,
    active: bool,
    p: &'static Palette,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let (background, foreground) = if active {
        (p.surface, p.primary)
    } else {
        (p.outline_variant, p.text_secondary)
    };
    h_flex()
        .id(id)
        .items_center()
        .justify_center()
        .py(px(9.))
        .rounded(px(RADIUS_SM))
        .bg(rgb(background))
        .text_size(px(13.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(foreground))
        .child(SharedString::from(label))
        .on_click(on_click)
}
