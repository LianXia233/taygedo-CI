//! 连接状态视图：仅在服务不可达时展示明确错误与排查指引。
//!
//! 本视图不收集任何凭据（无口令输入、无登录表单），只提供「重试连接」。

use gpui::{
    Context, Entity, IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Window, div, px, rgb,
};

use crate::state::{AppState, Connection};

use super::{ACCENT, MUTED, TEXT, action_button, card};

pub struct ConnectionView {
    state: Entity<AppState>,
}

impl ConnectionView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state }
    }
}

impl Render for ConnectionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let (title, detail) = match &state.connection {
            Connection::Failed(message) => ("无法连接本机回环服务", message.clone()),
            Connection::Connecting => (
                "正在连接本机回环服务",
                "正在探测 /api/meta，请稍候…".to_string(),
            ),
            Connection::Connected => ("服务已连接", "本机回环 API 可达。".to_string()),
        };
        let base = state.base_url.clone();

        div()
            .size_full()
            .flex_col()
            .gap(px(12.))
            .child(
                card()
                    .child(
                        div()
                            .text_size(px(16.))
                            .text_color(rgb(TEXT))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(super::DANGER))
                            .child(detail),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(format!("当前探测地址：{base}/api/meta")),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(
                                "排查：请确认 taygedo-rs（Windows 为 taygedo-rs.exe）位于本程序同目录，且以 desktop 形态构建（编译期免鉴权、仅监听 127.0.0.1、仅回环来源放行）；也可先手动启动服务，或设置 TAYGEDO_API_BASE 指向已运行的本机回环服务。",
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(MUTED))
                            .child(
                                "远程 / LAN 访问不在 Windows 桌面形态支持范围内，请使用 Linux / Docker 的 webui 形态。",
                            ),
                    )
                    .child(
                        action_button("重试连接", ACCENT, 0xffffff).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.bootstrap(cx));
                            }),
                        ),
                    ),
            )
    }
}
