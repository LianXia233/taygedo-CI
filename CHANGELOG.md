# 更新日志 (Changelog)

本项目的所有值得注意的变更都会记录在此文件中。

格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [0.2.1] - 2026-08-21

### 新增
- WebUI 背景壁纸（毛玻璃卡片 + 深浅色遮罩自适应）。
- LuCI 配置对齐 WebUI：金币任务、云异环时长、分享平台等开关。
- LuCI 新增「打开 WebUI」菜单项与状态页跳转按钮。

### 变更
- Rust 支持从环境变量初始化全局配置（`TAYGEDO_DEFAULT_SCHEDULE` / `TAYGEDO_COIN_TASKS` / `TAYGEDO_CLOUD_DURATION` / `TAYGEDO_SHARE_PLATFORM`），供 OpenWrt init.d 从 UCI 传入。

## [0.2.0] - 2026-08-21

### 新增
- WebUI 登录鉴权：改为**账号 + 密码**登录，默认 `admin / admin`，登录后可在「设置」中修改账号密码。
- 手机 / PC **响应式** WebUI（移动端单列、触摸友好，桌面端双列）。
- **OpenWrt / LuCI 集成**：`luci-app-taygedo` 包（UCI 配置、procd 守护、LuCI 状态页/配置页、菜单与 ACL）。
- **GitHub Actions 多平台自动编译**：Windows（`.zip`）、Debian（`.deb`）、OpenWrt（`.ipk` + `.apk`，x86_64 / aarch64），打 tag 或手动触发即发布到 Releases。

### 变更
- 登录方式由单一密码改为账号 + 密码。
- 首次启动若未设置密码，默认使用 `admin`（此前为随机生成），可用 `TAYGEDO_WEB_PASSWORD` 覆盖。

## [0.1.0] - 2026-08-21

### 新增
- 用 Rust 重写塔吉多自动签到核心逻辑（axum + tokio + reqwest）。
- 多账号、密码 / 短信验证码登录、每日定时签到（每账号独立时间）。
- WebUI 管理界面：账号管理、签到时间设置、手动签到、实时日志、深浅色主题。
- 完整签到链路：APP 签到、逐游戏签到（幻塔 1256 / 异环 1289 中文名）、金币任务、云异环时长。
- 幽灵角色修复（优先战绩卡）、会话自动续期（refreshToken → laohuToken → 密码重登）。
- `accounts.json` 与上游 TypeScript 版本完全兼容，可直接复用登录态。
