//! 主题调色板：逐项对齐 WebUI 的 `--md-*` 设计令牌。
//!
//! GPUI 没有 CSS 变量，色值只能以常量形式下发。这里刻意保留 WebUI 的令牌名
//! （surface / outline / primary / success / warning / danger），这样两套界面
//! 在语义上同构：改一处配色，另一边能立刻找到对应项，不会出现"网页是蓝的、
//! 桌面是紫的"这种漂移。

/// 一套配色。字段语义与 WebUI 的 CSS 变量一一对应。
#[derive(Clone, Copy)]
pub struct Palette {
    /// 页面底色（`--md-surface-dim` 的更暗一档，WebUI 用 `#f6f8fc`）。
    pub bg: u32,
    /// 卡片 / 页头底色（`--md-surface`）。
    pub surface: u32,
    /// 卡片内嵌行底色（`.card-row`）。
    pub surface_sunken: u32,
    /// 描边（`--md-outline`）。
    pub outline: u32,
    /// 弱描边 / 标签底色（`--md-outline-variant`）。
    pub outline_variant: u32,

    pub text: u32,
    pub text_secondary: u32,
    pub text_tertiary: u32,

    pub primary: u32,
    /// 主色淡底（`--md-primary-container`）。
    pub primary_soft: u32,
    /// 主色容器上的文字（`--md-on-primary-container`）。
    pub primary_ink: u32,
    pub on_primary: u32,

    pub success: u32,
    pub success_soft: u32,
    pub success_ink: u32,

    pub warning: u32,
    pub warning_soft: u32,
    pub warning_ink: u32,

    pub danger: u32,
    pub danger_soft: u32,

    /// 日志面板底色（WebUI 固定深色，不随主题变浅）。
    pub log_bg: u32,
    pub log_text: u32,
    pub log_ts: u32,
    pub log_info: u32,
    pub log_warn: u32,
    pub log_err: u32,
}

/// 浅色：与 WebUI `:root` 同值。
pub const LIGHT: Palette = Palette {
    bg: 0xf6f8fc,
    surface: 0xffffff,
    surface_sunken: 0xf8fafc,
    outline: 0xe2e8f0,
    outline_variant: 0xf1f5f9,

    text: 0x1e293b,
    text_secondary: 0x64748b,
    text_tertiary: 0x94a3b8,

    primary: 0x3b66f5,
    primary_soft: 0xedf2fe,
    primary_ink: 0x0f2b82,
    on_primary: 0xffffff,

    success: 0x10b981,
    success_soft: 0xecfdf5,
    success_ink: 0x047857,

    warning: 0xf59e0b,
    warning_soft: 0xfffbeb,
    warning_ink: 0xb45309,

    danger: 0xef4444,
    danger_soft: 0xfef2f2,

    log_bg: 0x0f172a,
    log_text: 0xe2e8f0,
    log_ts: 0x64748b,
    log_info: 0x38bdf8,
    log_warn: 0xfbbf24,
    log_err: 0xf43f5e,
};

/// 深色：与 WebUI `[data-theme="dark"]` 同值。
pub const DARK: Palette = Palette {
    bg: 0x090d16,
    surface: 0x1e293b,
    surface_sunken: 0x0f172a,
    outline: 0x334155,
    outline_variant: 0x1e293b,

    text: 0xf8fafc,
    text_secondary: 0x94a3b8,
    text_tertiary: 0x64748b,

    primary: 0x6366f1,
    primary_soft: 0x1e1b4b,
    primary_ink: 0xc7d2fe,
    on_primary: 0xffffff,

    success: 0x10b981,
    success_soft: 0x064e3b,
    success_ink: 0x6ee7b7,

    warning: 0xf59e0b,
    warning_soft: 0x78350f,
    warning_ink: 0xfde68a,

    danger: 0xef4444,
    danger_soft: 0x7f1d1d,

    log_bg: 0x090d16,
    log_text: 0xe2e8f0,
    log_ts: 0x64748b,
    log_info: 0x38bdf8,
    log_warn: 0xfbbf24,
    log_err: 0xf43f5e,
};

/// 圆角与间距：WebUI 的 `--md-radius-*` 是 10 / 14 / 20 / 24。
pub const RADIUS_SM: f32 = 10.;
pub const RADIUS_MD: f32 = 14.;
pub const RADIUS_CARD: f32 = 20.;
pub const RADIUS_MODAL: f32 = 24.;
