# 更新日志 (Changelog)

本文件记录塔吉多自动签到（Rust 版）的所有版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 各版本的完整代码差异可对比 Git Tag：`v0.1.0` … `v0.4.11`。

## [未发布]

### 文档
- **README 突出简单易用、零门槛**：导语改为强调「下载即用 + WebUI 图形化操作」的产品定位；新增「三步上手（零门槛）」章节，以表格给出 Windows / Debian / OpenWrt 三平台的下载 → 运行 → 使用最小路径（双击运行、两条命令、装包启用），并说明全程仅需浏览器操作、无需命令行与配置文件；功能特性列表新增「开箱即用，零门槛」条目。内容均按现有实际功能描述，未涉及代码变更。
- **README 移除未实际发布的架构描述**：「多平台 / 多架构」表格删除 `arm_cortex-a7/a9`（armv7 musl）、`mipsel_24kc`、`mips_24kc` 三行及 mips/mipsel 需改用 `native-tls` 的说明——CI 实际仅构建并发布 x86_64 与 aarch64 两个 musl 架构，文档与实际发布物保持一致。

## [0.4.11] - 2026-08-31

### 修复
- **APP 签到恒失败（`appSignin：invalid request`，HTTP 200 / code=22）**：`app_signin` 此前手工拼装了一套残缺的 native 请求头（`appversion: 1.1.0`，缺 `ds`、`platform`、`Accept`）。实测确认本端点**强制要求 `ds` 签名头**——缺失时上游一律返回 `code=22 invalid request`，而补齐 `ds` 后立即返回 `code=0`；`appversion` / `platform` / `Accept` 均非必需项。现已与同一端点的 `bbs_signin`（`communityId=2`）统一复用 `native_common_headers`，不再手工构造。该失败会导致整轮签到判定为 `failed`。
- **会话续期接口同源失效（`refreshToken` 缺 `ds` → code=22，阻断 laohuToken 兜底）**：`refresh_token` 同样手工拼装残缺头（缺 `ds`），上游恒返回 `code=22 invalid request`。该错误文案不含 `REFRESH_REJECTED_402`，导致 `refresh_or_rebuild_session` 将其误判为「非 402」而跳过 `laohuToken` 重建、直接失败。补齐 `ds` 后，refreshToken 失效时上游正确返回 `402`，进而落到 `laohuToken` 重建路径（已实测 `user_center_login` 可用）。手工头中的 `appversion: 1.1.0` 一并改为 `native_common_headers` 的 `1.2.5`。
- **会话过期导致部分端点被网关直接拒签（HTTP 401）**：原逻辑「本地已有 `accessToken` 就直接使用」，而 `accessToken` 会过期；过期后网关层（istio-envoy）对 `getUserTasks`、`/apihub/awapi/*`、`/bbs/api/post/*` 等端点直接返回 HTTP 401（空响应体、1ms 内返回，未达后端），而 `code=22` 这类业务错误又不会命中 `is_auth_error`，导致当天签到永久失败且无自愈路径。现改为签到前**主动续期**（`refreshToken` 代价最小、不触发风控），续期失败时回退复用现有会话并由后续重试兜底，避免弱网下误伤。
- **金币任务帖子列表解析失败（浏览 / 点赞 / 分享恒为 0）**：`get_recommend_post_list` 原仅识别 `data.list` 或 `data` 为数组，而上游实际返回 `data` 为对象、帖子位于 `data.posts`（`{"code":0,"data":{"hasMore":true,"page":2,"posts":[...]}}`），导致解析恒为 `None`，日志报「获取帖子列表失败：…（HTTP 200，code=0，msg=ok）」这一自相矛盾的错误，金币任务中除 BBS 签到外的浏览 / 点赞 / 分享子任务全部无法执行。现按 `data.list` → `data.posts` → `data` 顺序依次尝试，兼容多种响应形态（已实测帖子的 `postId` 与 `selfOperation.liked` 字段与现有取值逻辑匹配）。

### 变更
- **APP 签到失败不再中断整轮**：原实现用 `?` 传播 `app_signin` 的错误，导致这一独立子任务失败后，游戏签到、金币任务、云时长全部不执行（故障放大器）。现改为降级记录并继续执行；`app_signin` 字段在无结果时为 `null`，失败原因合入 `error` 字段。状态判定：全部游戏失败，或 APP 签到失败且无任何游戏成功时，才判为 `failed`。
- **版本号同步**：bump 至 `v0.4.11`，`Cargo.toml` / `Cargo.lock` 同步更新。

## [0.4.10] - 2026-08-25

### 修复
- **跨平台编译失败修复（v0.4.9 无法构建）**：v0.4.9 在 Windows / Debian / Linux-musl 全平台 `cargo build --release --locked` 均失败（共 5 处 Rust 编译错误），导致 OpenWrt 与 Release 发布任务被跳过。本次修复内容：
  - **密码重登兜底条件匹配错误（E0308）**：`refreshToken` 失效与 `laohuToken` 重建失败两条兜底路径中，`if let Some((phone, pwd))` 错误匹配了裸元组 `(Option<&str>, Option<&str>)`，实际应匹配 `Option<...>`；改为 `if let (Some(phone), Some(pwd))`，仅在手机号与密码均存在时执行密码重登。
  - **`GameSigninResult` 缺少 `error` 字段（E0560 / E0609）**：单游戏签到结果结构体未定义 `error` 字段，但构造与结果展示处均在使用；已补充 `error: Option<String>`（序列化时自动省略 `None`，兼容既有 WebUI / LuCI 前端）。
  - **`updated_accounts` 借用冲突（E0502）**：签到结果写回时，`updated_by_id` 持有 `updated_accounts` 中 `&str` 的不可变借用，与 `updated_accounts[idx] = updated` 的可变写入冲突；索引表 key 改为 `String`（clone id）后解除借用。
- **版本号同步**：bump 至 `v0.4.10`，`Cargo.toml` / `Cargo.lock` 同步更新。

## [0.4.9] - 2026-08-25

### 修复
- **单游戏签到失败不再中断整轮签到**：原逻辑下某个游戏请求失败会直接中断该账号后续所有游戏任务；现改为逐个游戏独立执行并收集失败结果，单个失败不再影响其他游戏与账号。
- **配置文件损坏不再静默清空**：`config.json` 解析失败时原逻辑会直接覆写为空配置，导致账号等业务数据丢失；现改为原子写入（临时文件 + 重命名），并增加损坏备份（`.bak`）与失败日志，损坏时保留现场便于恢复。
- **会话重建顺序修正**：登录态失效后的重建顺序由「refreshToken 优先」调整为「密码重登优先、refreshToken 兜底」，避免 refreshToken 已失效时仍按旧会话继续、导致密码重登逻辑误判。
- **时间校验逻辑收敛**：三份重复的时间格式 / 范围校验函数统一为一处（`time.rs`），消除行为不一致隐患。

### 变更
- **免鉴权模式（`TAYGEDO_NO_AUTH`）仅 OpenWrt 平台生效**：非 OpenWrt 平台强制关闭免鉴权，防止误开导致服务暴露；免鉴权模式下修改密码接口直接拒绝，LuCI 同步隐藏改密区块。
- **LuCI 与 WebUI 职责解耦**：UCI 仅管理服务级配置（监听地址 / 端口 / 免鉴权开关），业务级配置（默认签到时间 / 金币任务 / 云时长 / 分享平台）统一由 `config.json` 管理，WebUI 与 LuCI 读写同一份业务数据，互不覆盖。

## [0.4.8] - 2026-08-23

### 变更
- **账号卡片标题显示平台昵称**：卡片标题与日志筛选标签从备注名（如“主账号”）改为平台昵称 `role_name`（如“恋夏233”），无昵称时回退备注名；原角色名标签与标题重复，已移除。WebUI 与 LuCI 同步更新。

## [0.4.7] - 2026-08-23

### 修复
- **升级后 WebUI 仍显示旧界面**：`/` 响应头无任何缓存控制，浏览器按启发式缓存旧版 `ui.html`（旧二进制内嵌的 v0.4.4 之前界面：文字头像、旧卡片），导致升级后出现"WebUI 与 LuCI 元素不一致/头像不同"的假象。现在 `index` 路由返回 `Cache-Control: no-cache`，每次进入页面都重新拉取，升级即刻生效。两端的头像图片本身完全相同（同一 92x92 PNG base64）。

## [0.4.6] - 2026-08-23

### 修复
- **LuCI 完整界面渲染后被回退为免鉴权引导页**：`startPoll()` 引用了 `TGD` 闭包私有的 `pollTimer` 导致 `ReferenceError`，被探测的 catch 误判为"未开免鉴权"，刚渲染好的主界面立即被引导页覆盖。已将 `pollTimer` 提升至模块作用域，并把探测失败与渲染异常的错误处理拆分为两条链路，渲染异常不再误入引导页。
- **WebUI 免鉴权探测与 LuCI 同步**：WebUI 启动探测从 `/api/meta` 改为 `/api/config`（免鉴权时裸请求即 200、需鉴权时 401）。部分运行中的二进制没有 `/api/meta` 路由（404），会导致免鉴权模式下 WebUI 误入登录页。

## [0.4.5] - 2026-08-22

### 变更
- **自定义头像图片**：账号卡片头像从首字母文字改为自定义 PNG 图片（base64 内嵌，无需后端改动），WebUI 与 LuCI 前端同步更新。

## [0.4.4] - 2026-08-22

### 变更
- **LuCI 前端同步 WebUI v0.4.3 视觉**：账号卡片与运行日志筛选标签栏（按账号独立显示）同步到 LuCI 独立版（aurora 主题），复用主题 CSS 变量，随亮/暗色自动切换。

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
