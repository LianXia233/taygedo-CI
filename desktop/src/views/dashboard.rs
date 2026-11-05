//! 主面板：账号列表、刷新、手动签到、新增/删除账号、签到时间、运行日志。

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};
use gpui_component::{
    h_flex,
    input::{Input, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};

use crate::client::{AccountRow, AddAccountRequest};
use crate::state::AppState;

use super::{ACCENT, ACCENT_SOFT, DANGER, LINE, MUTED, OK, SURFACE, TEXT, action_button, card};

pub struct DashboardView {
    state: Entity<AppState>,
    phone: Entity<InputState>,
    password: Entity<InputState>,
    captcha: Entity<InputState>,
    name: Entity<InputState>,
    schedule: Entity<InputState>,
    mode_captcha: bool,
    schedule_target: Option<String>,
}

impl DashboardView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        let phone = cx.new(|cx| InputState::new(&mut *window, cx).placeholder("手机号"));
        let password = cx.new(|cx| {
            InputState::new(&mut *window, cx)
                .placeholder("账号密码（密码模式）")
                .masked(true)
        });
        let captcha =
            cx.new(|cx| InputState::new(&mut *window, cx).placeholder("短信验证码（验证码模式）"));
        let name = cx.new(|cx| InputState::new(&mut *window, cx).placeholder("备注名称（可选）"));
        let schedule =
            cx.new(|cx| InputState::new(&mut *window, cx).placeholder("HH:MM，留空恢复默认"));
        Self {
            state,
            phone,
            password,
            captcha,
            name,
            schedule,
            mode_captcha: false,
            schedule_target: None,
        }
    }

    fn account_row(&self, row: &AccountRow, cx: &mut Context<Self>) -> gpui::Div {
        let id_signin = row.id.clone();
        let id_delete = row.id.clone();
        let id_schedule = row.id.clone();
        let schedule_value = row.schedule.clone();
        v_flex()
            .gap(px(6.))
            .p(px(12.))
            .rounded(px(8.))
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(LINE))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(rgb(TEXT))
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(row.phone.clone().unwrap_or_default()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(row.role_name.clone().unwrap_or_default()),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(if row.signed_today {
                                rgb(OK)
                            } else {
                                rgb(MUTED)
                            })
                            .child(if row.signed_today {
                                "今日已签"
                            } else {
                                "今日未签"
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(12.))
                    .items_center()
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(format!("UID：{}", row.uid)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(format!("签到时间：{}", row.schedule)),
                    )
                    .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                        if row.has_password {
                            "已存密码"
                        } else {
                            "仅令牌"
                        },
                    )),
            )
            .child(
                h_flex()
                    .gap(px(8.))
                    .child(
                        action_button("签到", ACCENT, 0xffffff).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.signin(cx, id_signin.clone()));
                            },
                        )),
                    )
                    .child(
                        action_button("设置时间", ACCENT_SOFT, ACCENT).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.schedule_target = Some(id_schedule.clone());
                                this.schedule.update(cx, |input, cx| {
                                    input.set_value(schedule_value.clone(), window, cx);
                                });
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        action_button("删除", 0xfee2e2, DANGER).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.delete_account(cx, id_delete.clone())
                                });
                            },
                        )),
                    ),
            )
    }
}

impl Render for DashboardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let accounts = state.accounts.clone();
        let logs = state.logs.clone();
        let today = state.today.clone();
        let busy = state.busy;
        let editing = self.schedule_target.clone();

        let mut account_rows: Vec<gpui::AnyElement> = Vec::new();
        for row in &accounts {
            account_rows.push(self.account_row(row, cx).into_any_element());
        }
        if account_rows.is_empty() {
            account_rows.push(
                div()
                    .p(px(16.))
                    .text_size(px(13.))
                    .text_color(rgb(MUTED))
                    .child("暂无账号，请在下方添加。")
                    .into_any_element(),
            );
        }

        let schedule_editor = if let Some(editing_id) = editing {
            h_flex()
                .gap(px(8.))
                .items_center()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child("签到时间（HH:MM，留空恢复默认）："),
                )
                .child(div().w(px(160.)).child(Input::new(&self.schedule)))
                .child(
                    action_button("保存", ACCENT, 0xffffff).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let id = editing_id.clone();
                            let value = this.schedule.read(cx).value().trim().to_string();
                            let time = if value.is_empty() { None } else { Some(value) };
                            this.schedule_target = None;
                            this.state
                                .update(cx, |state, cx| state.set_schedule(cx, id, time));
                        },
                    )),
                )
                .child(
                    action_button("取消", ACCENT_SOFT, ACCENT).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.schedule_target = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let mut log_rows: Vec<gpui::AnyElement> = Vec::new();
        for log in logs.iter().rev().take(12) {
            log_rows.push(
                h_flex()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(div().w(px(160.)).child(log.ts.clone()))
                    .child(div().w(px(56.)).child(log.level.clone()))
                    .child(div().flex_1().child(log.message.clone()))
                    .into_any_element(),
            );
        }
        if log_rows.is_empty() {
            log_rows.push(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("暂无运行日志。")
                    .into_any_element(),
            );
        }

        v_flex()
            .size_full()
            .gap(px(12.))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .text_color(rgb(TEXT))
                            .child(format!(
                                "账号管理（{} 个）· {} · {}",
                                accounts.len(),
                                today,
                                if busy { "处理中" } else { "空闲" }
                            )),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(
                                action_button("刷新", ACCENT, 0xffffff).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| state.refresh(cx));
                                    },
                                )),
                            )
                            .child(
                                action_button("全部签到", OK, 0xffffff).on_click(cx.listener(
                                    |this, _, _, cx| {
                                        let ids: Vec<String> = this
                                            .state
                                            .read(cx)
                                            .accounts
                                            .iter()
                                            .map(|account| account.id.clone())
                                            .collect();
                                        this.state.update(cx, |state, cx| {
                                            for id in ids {
                                                state.signin(cx, id);
                                            }
                                        });
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                card().flex_1().min_h_0().child(schedule_editor).child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex_col()
                        .gap(px(8.))
                        .children(account_rows)
                        .overflow_y_scrollbar(),
                ),
            )
            .child(
                card()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT))
                            .child("新增账号"),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(div().w(px(200.)).child(Input::new(&self.phone)))
                            .child(div().w(px(170.)).child(Input::new(&self.name)))
                            .child(
                                action_button(
                                    if self.mode_captcha {
                                        "模式：验证码登录"
                                    } else {
                                        "模式：密码登录"
                                    },
                                    ACCENT_SOFT,
                                    ACCENT,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.mode_captcha = !this.mode_captcha;
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .items_center()
                            .child(div().w(px(200.)).child(Input::new(&self.password)))
                            .child(div().w(px(170.)).child(Input::new(&self.captcha)))
                            .child(action_button("发送验证码", ACCENT_SOFT, ACCENT).on_click(
                                cx.listener(|this, _, _, cx| {
                                    let phone = this.phone.read(cx).value().trim().to_string();
                                    if phone.is_empty() {
                                        this.state.update(cx, |state, cx| {
                                            state.status = "请先填写手机号".to_string();
                                            state.status_error = true;
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    this.state
                                        .update(cx, |state, cx| state.send_code(cx, phone));
                                }),
                            ))
                            .child(action_button("添加账号", ACCENT, 0xffffff).on_click(
                                cx.listener(|this, _, _, cx| {
                                    let phone = this.phone.read(cx).value().trim().to_string();
                                    let password =
                                        this.password.read(cx).value().trim().to_string();
                                    let captcha = this.captcha.read(cx).value().trim().to_string();
                                    let name = this.name.read(cx).value().trim().to_string();
                                    let mode = if this.mode_captcha {
                                        "captcha"
                                    } else {
                                        "password"
                                    };
                                    if phone.is_empty() {
                                        this.state.update(cx, |state, cx| {
                                            state.status = "手机号不能为空".to_string();
                                            state.status_error = true;
                                            cx.notify();
                                        });
                                        return;
                                    }
                                    let req = AddAccountRequest {
                                        phone,
                                        mode: mode.to_string(),
                                        password: (!password.is_empty()).then_some(password),
                                        captcha: (!captcha.is_empty()).then_some(captcha),
                                        name: (!name.is_empty()).then_some(name),
                                    };
                                    this.state
                                        .update(cx, |state, cx| state.add_account(cx, req));
                                }),
                            )),
                    ),
            )
            .child(
                card()
                    .h(px(210.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(TEXT))
                            .child("运行日志（最近 12 条）"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex_col()
                            .gap(px(2.))
                            .children(log_rows)
                            .overflow_y_scrollbar(),
                    ),
            )
    }
}
