# 更新日志 (Changelog)

本文件记录塔吉多自动签到（Rust 版）的所有版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 各版本的完整代码差异可对比 Git Tag：`v0.1.0` … `v0.4.3`。

## [0.4.3] - 2026-08-22

### 变更
- **账号卡片视觉重构**：头像改为渐变圆角方块（带阴影），名称与状态徽章分行显示；手机号、UID、角色名改为独立标签（meta-tag），各带对应图标，角色名标签高亮为主题色；卡片 hover 时顶部出现渐变色条。
- **运行日志按账号筛选**：日志面板新增筛选标签栏，支持「全部」+ 各账号独立查看；点击标签即时过滤日志内容（匹配账号名 / ID / UID 关键词），多账号场景下各账号签到记录一目了然。

## [0.4.2] - 2026-08-21

### 修复
- **定时签到失效修复（全平台）**：调度器原逻辑仅做「精确分钟匹配」，若进程在该 1 分钟窗口内未运行（如 Windows 开机较晚、服务 / 容器重启），当天签到会永久错过。改为「到时间即触发 + 补签」：账号设定时间已过（或正好到达）且当天尚未触发时立即签到；每个账号每天仅触发一次，跨天自动重置。

### 新增
- **Windows 启动自动打开 WebUI**：Windows 桌面端程序启动后自动用默认浏览器打开管理界面（服务器 / OpenWrt / Docker 无桌面环境不触发），无需手动复制地址。

## [0.4.1] - 2026-08-22

### 变更
- **CLI 启动横幅优化**：访问地址改为可点击的本地 URL（绑定 `0.0.0.0` 时显示 `http://127.0.0.1:port`），同时保留监听所有接口的提示信息。
- 启动横幅新增**鉴权状态行**：免鉴权模式显示「免鉴权模式 (无需登录)」，否则显示默认账号提示。
- 统一启动信息输出位置（移除 `service.rs` 中散落的默认密码 `println!`，全部整合到主横幅），Windows / Debian / OpenWrt 各平台 CLI 显示一致。

## [0.4.0] - 2026-08-22

### 变更
- **LuCI 独立重构**：移除 LuCI 端登录态 / token / 自动登录逻辑，页面进入即以免鉴权模式直连后端 API（先探测 `GET /api/meta` 的 `no_auth` 字段，为真直接渲染主界面，否则提示页引导跳转外部 WebUI）。
- LuCI 头部新增「外部 WebUI」按钮，一键打开 `:port` 独立管理界面。

### 修复
- **CI 修复**：`Cargo.toml` 版本升至 `0.4.0` 时未同步 `Cargo.lock`，导致原生 glibc / Windows 构建在 `cargo build --release --locked` 报 `cannot update the lock file ... --locked was passed` 而失败（musl `cross` 构建因工具链较宽松侥幸通过）。本版本已将 lock 中根包 `taygedo-rs` 的版本由 `0.3.0` 更正为 `0.4.0`，依赖图完全不变。

## [0.3.1] - 2026-08-22

### 修复
- 修复 `auth_middleware` 免鉴权放行逻辑（`no_auth` 短路），确保 `TAYGEDO_NO_AUTH=1` 时所有 API 正确免登录。

## [0.3.0] - 2026-08-22

### 新增
- **免鉴权模式**：新增 `TAYGEDO_NO_AUTH` 环境变量（OpenWrt UCI `option no_auth`），开启后 WebUI / LuCI / 所有 API 均无需登录即可直接使用，适合内网自用。
- 新增公开接口 `GET /api/meta` 返回服务元信息（含 `no_auth` 状态），WebUI 与 LuCI 前端据此自动跳过登录步骤。

### 变更
- OpenWrt UCI 默认配置新增 `no_auth` 选项（默认 `1` 开启）；init.d 自动透传 `TAYGEDO_NO_AUTH`。
- 版本号统一升至 0.3.0（Cargo.toml / Makefile）。

## [0.2.7] - 2026-08-22

### 新增 / 变更
- **LuCI 独立免鉴权前端**：重构 `status.js`，与 WebUI 代码解耦；进入即直接使用、无登录框。
- LuCI 新增「跳转 WebUI」按钮（页面右上角），一键打开独立 WebUI（默认 `:8787`）。
- 功能对齐 WebUI：账号管理、密码 / 短信验证码登录、每日签到时间、立即签到、运行日志、全局设置、修改密码。

## [0.2.5] - 2026-08-21

### 新增 / 变更
- **LuCI 前端从 Lua 重写为现代 JS**（`htdocs/luci-static/resources/view/taygedo/status.js`），功能与 WebUI 完全一致：账号管理、密码/短信验证码登录、每日签到时间、立即签到、运行日志、全局设置、修改密码。
- LuCI 已由 OpenWrt root 鉴权保护，进入页面**自动静默登录后端**（用 UCI `web_password`，默认 `admin`），无需二次输入密码；仅当后端密码与 UCI 不同步时才兜底显示登录框。
- 删除 `luasrc/` 下三个 Lua 文件（controller / model/cbi / view 模板）。
- Rust 后端新增 **CORS 中间件**，允许 LuCI（不同端口）跨源调用 API。
- WebUI 美化：品牌渐变标题、卡片/统计卡片 hover 微动效、入场动画、按钮质感提升。
- Makefile / package.sh 改为安装 `htdocs` 到 `/www/luci-static`。

## [0.2.4] - 2026-08-21

### 修复
- 修复 OpenWrt `.apk` 打包格式：原手工拼接 `gzip(PKGINFO)+gzip(data)` 是 apk **v2** 格式，OpenWrt 24.10+ 的 apk-tools 3.x 无法安装（报 `v2 package format error`）。现改用 apk-tools 3.0 的 `apk mkpkg` 生成正确的 **v3 ADB** 格式包。
- 修复 `.ipk` 打包格式：由 gzip-tar 改为标准 `ar` 归档（`debian-binary` + `control.tar.gz` + `data.tar.gz`），opkg 可正常安装。
- 修复 LuCI 包 Makefile 中错误的下载仓库地址（`taygedo-auto-attendance-rs` → `taygedo-CI`），并同步版本号到 0.2.4。

### 验证
- 在 ImmortalWrt SNAPSHOT（apk-tools 3.0.5）上实测：`apk mkpkg` 生成的包通过 `apk verify`，并能以 `--allow-untrusted` 成功安装。

## [0.2.3] - 2026-08-21

### 修复
- 修复 GitHub Actions 打包三处错误：
  - Windows 打包改用 PowerShell `Compress-Archive`（原 `zip` 命令在 Windows runner 不存在）。
  - `.deb` 打包修正 `DEBIAN` 目录名（原小写 `debian`）与包根目录结构，且不再打包 LuCI 文件。
  - OpenWrt 打包脚本将输出/二进制路径转绝对路径（修复 `cd` 后相对路径失效）。
- 首次成功发布全平台安装包到 Releases：Windows `.zip`、Debian `.deb`、OpenWrt `.ipk` + `.apk`（x86_64 / aarch64）、Linux musl `.tar.gz`。

## [0.2.2] - 2026-08-21

### 修复
- 修复 CI 打包脚本（`dpkg-deb` 路径、Windows zip）。该版本 OpenWrt 打包仍存在相对路径问题，由 0.2.3 修复。

## [0.2.1] - 2026-08-21

### 新增
- WebUI 背景壁纸（毛玻璃卡片 + 深浅色遮罩自适应）。
- LuCI 配置对齐 WebUI：金币任务、云异环时长、分享平台等开关。
- LuCI 新增「打开 WebUI」菜单项与状态页跳转按钮。

### 变更
- Rust 支持从环境变量初始化全局配置（`TAYGEDO_DEFAULT_SCHEDULE` / `TAYGEDO_COIN_TASKS` / `TAYGEDO_CLOUD_DURATION` / `TAYGEDO_SHARE_PLATFORM`），供 OpenWrt init.d 从 UCI 传入。

## [0.2.0] - 2026-08-21

### 新增
- WebUI 登录鉴权：改为**账号 + 密码**登录，默认 `admin / admin`，登录后可修改。
- 手机 / PC **响应式** WebUI。
- **OpenWrt / LuCI 集成**。
- **GitHub Actions 多平台自动编译**并发布 Releases。

### 变更
- 登录方式由单一密码改为账号 + 密码。
- 首次启动默认密码 `admin`（可用 `TAYGEDO_WEB_PASSWORD` 覆盖）。

## [0.1.0] - 2026-08-21

### 新增
- 用 Rust 重写塔吉多自动签到核心逻辑（axum + tokio + reqwest）。
- 多账号、密码 / 短信验证码登录、每日定时签到。
- WebUI 管理界面、完整签到链路、幽灵角色修复、会话自动续期。
- `accounts.json` 与上游 TypeScript 版本完全兼容。
