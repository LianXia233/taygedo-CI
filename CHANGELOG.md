# 更新日志 (Changelog)

本文件记录塔吉多自动签到（Rust 版）的所有版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 各版本的完整代码差异可对比 Git Tag：`v0.1.0` … `v0.7.3`。

> **关于初始口令的重要提示**：`admin / admin` 这类固定初始口令**仅存在于 0.5.0 之前的历史版本**，
> 下方旧版本条目（如 0.2.0）中的相关描述是当时的事实记录，**不代表当前行为**。
> 0.5.0 起首次启动一律由 CSPRNG 随机生成初始口令，仅在启动横幅打印一次。
> 以 [README](README.md) 的说明为准。

## [Unreleased]

## [0.7.3] - 2026-11-03

### 新增 · 成品 Docker 镜像发布到 GHCR

`docker.yml` 在 `v*` tag / `main` / 手动触发时构建 `linux/amd64` + `linux/arm64`
双架构镜像并推送到 `ghcr.io/lianxia233/taygedo-rs`，不再需要目标机器装 Rust 工具链。

- 标签体系：`latest` / `<版本>` / `<主>.<次>` / `edge`（跟随 main）/ `sha-<短哈希>`。
- 基础镜像仍为 Debian `bookworm-slim`，与 `.deb` 共用 glibc 运行时；
  相比旧 Dockerfile 多做了四件事：依赖编译分层缓存（改业务代码不重编第三方
  crate）、非 root 运行（uid/gid 10001）、`HEALTHCHECK` 探活 `/api/meta`、
  以及构建期清空 `.cargo/config.mirror.toml`（海外 runner 直连 crates.io）。
- `docker-compose.yml` 默认改为拉取 GHCR 成品镜像，本地构建降到「方式 B」。
- 镜像内含 `wget`（仅用于健康检查），不装 curl、不装 ca-certificates
  ——reqwest 用 rustls + webpki-roots，根证书已编进二进制。

## [0.7.2] - 2026-11-03

> **破坏性变更**：OpenWrt 产物不再包含 WebUI、登录/改密接口与前端加密资产，
> 入口只有 LuCI。Windows / Debian / Docker 不受影响（默认 `webui` 形态）。

### 解耦 · OpenWrt 与 Windows/Debian 改为编译期双形态

上一版的"平台分流"仍是**同一个二进制**在运行期按 `TAYGEDO_EDITION` /
`/etc/openwrt_release` 判断，OpenWrt 上还留着 `404 webui_retired` /
`410 deprecated_endpoint` 的占位路由。那不是解耦，只是"注册了再拒绝"：
WebUI 的路由、口令体系与全部前端资产仍然编进 OpenWrt 二进制。

现在改为 Cargo feature 互斥构建，形态在编译期固定：

| 形态 | 构建命令 | 首页 / 登录 / 改密 / 静态资源 | 业务鉴权 |
| :--- | :--- | :--- | :--- |
| `webui`（默认） | `cargo build --release` | 全部存在 | `Bearer token`（7 天） |
| `openwrt` | `--no-default-features --features openwrt` | **路由不存在（404）** | 受信来源 + LuCI 会话 |

- 删除 `is_openwrt()` 运行期探测（含 `TAYGEDO_EDITION` 与 `/etc/openwrt_release`
  判定），改为编译期常量 `IS_OPENWRT` / `EDITION`。形态不再能被环境变量改写。
- 删除 `webui_retired` 与 `deprecated_endpoint`：OpenWrt 路由表里根本没有 `/`、
  `/api/login`、`/api/logout`、`/api/password`、`/static/*`、
  `/api/crypto/handshake`、`/favicon.ico`。请求它们得到的是 axum 默认 404。
- `src/ui.html` 与 noble 系列前端资产在 `openwrt` 下不参与 `include_str!`，
  不进入二进制；`manifest::ASSETS` 该形态为空。
- OpenWrt 产物不挂加密信封中间件，`/api/meta` 不再返回无消费者的
  `crypto_proto` / `crypto_alg` / 身份指纹，改为返回 `entry: "luci"`。
- `AppState` 的 token 表、登录失败计数、初始口令字段只在 `webui` 编译；口令哈希
  （scrypt/v1 兼容）、token 签发、改密与锁定、`no_auth` 开关同样按形态归属。
- 打包与 CI：Release 增加 `taygedo-rs-<triple>-openwrt.tar.gz`，OpenWrt Makefile
  只下载该产物（避免把带 WebUI 的通用 musl 包装进路由器）；CI 对两种形态分别
  跑 fmt / clippy / test，并新增 OpenWrt 冒烟：验证六个 WebUI 入口均为 404、
  无 LuCI 会话 401、内部通道 200。
- `init.d` 移除 `TAYGEDO_EDITION=openwrt`，注释同步为"形态由二进制自身决定"。
- Docker 构建显式 `--features webui`，防止将来改动默认值时容器静默变成无界面。

### 恢复 · 独立部署的内置 WebUI（OpenWrt 维持 rpcd/ucode 现状）

> 本节描述的是上一版实现（运行期分流 + 404/410 占位），已被上一节的
> 编译期双形态取代，保留作为变更记录。

v0.7.1 把内置 WebUI 与自有口令体系整体删除。这个决定对 OpenWrt 成立（LuCI 是
入口，自有登录只会多一个可爆破面），但对 **Windows / Debian / Docker 属于误伤**：
这些平台没有别的图形入口，删掉 WebUI 等于把"打开浏览器就能用"变成"自己写客户端"。

本次按平台分流，把独立部署的 WebUI 按删除前的形态加回来，**OpenWrt 一行不改**：

| 形态 | `GET /` | 登录接口 | 业务鉴权 | 浏览器侧加密 |
| :--- | :--- | :--- | :--- | :--- |
| 独立部署 | 下发内置 WebUI | `/api/login`、`/api/logout`、`/api/password` | `Bearer token`（7 天） | 强制走 v2 信封 |
| OpenWrt | `404 webui_retired` | `410 deprecated_endpoint` | 受信来源 + LuCI 会话 | 无浏览器消费者 |

- 恢复 `src/ui.html`（White MD3 单文件界面）、`web_username` / `web_password_hash` /
  `web_password_salt` / `web_password_version` / `web_password_must_change` /
  `lan_no_auth` 配置项，以及 scrypt 口令哈希（v2）与 v1 就地升级路径。
- 登录失败 5 次锁定 5 分钟、恒定成本口令校验、改密后吊销全部会话，均按原实现恢复。
- 启动横幅恢复"访问地址 + 首启随机口令只打印一次"；Windows 桌面端启动后自动开浏览器。
- OpenWrt 上这些入口一律显式返回 404 / 410（而不是让请求落到含糊的 axum 404），
  `manifest.json` 同步剔除首页 `/`，与 v0.7.1 行为一致。

### 加密 · 补三处会让"加密看起来生效、实际不生效"的缺口

1. **改密响应送不出去**：`set_credentials` 会销毁全部加密会话，而响应还要用
   发起请求的那条会话加密。旧实现因此让客户端收到 `428 no_session`，误判改密
   失败，实际口令已经写盘。现在保留当前会话（`destroy_all_except`），其余照样作废。
   这与 `update_config` 改加密策略时的处理保持一致。
2. **首页字节必须与签名清单同源**：`GET /` 直接下发 `manifest::asset("/")` 的
   编译期字节，**不再**读取 `/etc/taygedo/ui.html` 之类的外部覆盖文件。允许运行期
   替换 HTML 时，清单哈希必然对不上，`web_guard.js` 会把整个界面拦死——等于给
   自己造了个"改个页面就登不进去"的坑。要改界面就重编：签名清单即可信字节的唯一来源。
3. **首启口令的可恢复性**：`TAYGEDO_WEB_PASSWORD` 现在是"显式优先"——设置即覆盖，
   不设置才沿用 UI 里改过的口令。这样忘记口令有确定恢复路径，自动化部署也能固定凭据
   （已与存储的哈希一致时不重写，避免每次启动换盐）。

### 测试 · E2E 覆盖登录全链路

`scripts/e2e_web.mjs` 恢复登录流程，并新增：未登录 401、首页与清单哈希同源、
错误口令 401（响应已解密）、改密成功响应可解密、旧 token 立即失效、新口令可登录、
登出后 token 失效。CI 的 E2E 步骤补回 `TAYGEDO_WEB_PASSWORD`。

### CI · 补齐审计作业的 `checks: write` 权限

`dependency-audit` 作业在 `v0.7.1` 发布构建中失败，日志为
`Resource not accessible by integration - checks/runs`：`rustsec/audit-check@v2`
需要创建 check run 来承载审计结论，而作业只拿到了默认的只读令牌。
**这是权限缺失，不是漏洞问题**——同一次运行的审计结论是 `vulnerabilities found: false`
（count 0），仅剩一条 `chacha20` yanked 提示（经 `quinn` 的 `http3` 特性进入锁文件，
本项目未启用该特性，该包不参与编译）。

修法是给该作业加作业级最小权限 `contents: read` + `checks: write`，不改工作流顶层
`permissions`、不放开其他作业、不动 `continue-on-error` 与失败阈值、不加 ignore 列表。
作业级 `permissions` 是整体覆盖而非追加，因此该作业的 `contents` 由顶层的 `write` 收窄为 `read`。

## [0.7.1] - 2026-11-03

### 概要

- **彻底移除内置 WebUI**：首页 `/`、登录 / 登出 / 改密接口、WebUI 专用静态资源与清单条目、
  `web_password` 等配置项一并删除，`GET /` 直接 404。
- **OpenWrt 改为 LuCI 经 rpcd/ucode 内部通道访问后端**：不再有浏览器侧加密前端，
  页面只调 `luci.taygedo`，由服务端 curl 回环 `127.0.0.1:<port>`。
- **鉴权边界收敛到 rpcd ACL**：会话号只认 rpcd 注入的服务端真值，方法粒度读写授权。
- **修复 ubus 会话解析与冷会话自调用死锁**：`parse_session_get` 兼容 `{"values":{"username":...}}`；
  后端在 OpenWrt + 回环路径不再嵌套 `ubus call session get`。

### CI · GitHub Actions 升级到 Node 24 运行时

GitHub Actions 已弃用 Node.js 20，`checkout@v4` / `setup-node@v4` / `upload-artifact@v4`
等步骤被强制在 Node 24 上运行并持续输出弃用告警。本次只做版本升级，不改动任何步骤语义
（inputs、缓存键、产物路径、artifact 名称、条件表达式、权限声明均保持原样）：

| action | 旧版本 | 新版本 | 说明 |
|---|---|---|---|
| `actions/checkout` | v4 | v7 | v5 起为 node24；v6 改了凭据存放位置（对 `git` 命令透明）；v7 仅限制 fork PR 在 `pull_request_target` / `workflow_run` 下默认签出，本仓库不用这两种触发 |
| `actions/setup-node` | v4 | v7 | v5 起为 node24；v6 的自动缓存只在 `package.json` 声明 npm 包管理时生效，v7 移除了 `registry-url` 上的空 token 回退（本步骤只设 `node-version`，无影响） |
| `actions/upload-artifact` | v4 | v7 | v6 起默认 node24，v7 新增可选 `archive: false` 直传，默认行为不变 |
| `actions/download-artifact` | v4 | v8 | v7 起默认 node24，v8 新增按摘要校验失败即报错（`digest-mismatch: error`），本仓库按名称下载，路径语义未变 |
| `softprops/action-gh-release` | v2 | v3 | v2.6.2 是最后一个 v2 版本且不再维护（node20），v3 迁移到 node24，inputs 未变 |

未升级项：`rustsec/audit-check@v2` 的 `v2` 分支仍是 `node20`，上游没有迁移到 node24 的
新 major（只有未发版的 `main`）。保持原样：`continue-on-error: true` 的非阻塞策略不变，
GitHub 会继续强制它在 Node 24 上运行、并在 `dependency-audit` 作业日志里保留该提示。

无需升级：`Swatinem/rust-cache@v2` 的 `v2` 分支已是 `node24`；`dtolnay/rust-toolchain@stable`
与 `taiki-e/install-action@v2` 是 composite action，不使用 Node 运行时。

### 破坏性变更 · 内置 WebUI 彻底移除，OpenWrt 改走内部 ucode/rpcd 通道

- **内置 WebUI 已删除**（不是"保留但不启用"）：`src/ui.html`、`/` 首页路由、
  `/api/login`、`/api/logout`、`/api/password` 登录 / 改密 / 登出接口、
  WebUI 专用静态资源与清单条目、`web_password` 等配置项与相关测试全部移除。
  `GET /` 由 axum 直接返回 `404`，不再有 `webui_retired`（404 带 code）或 `410 Gone`
  这类"保留但弃用"的形态。
- **没有应用自有凭据**：不再生成随机初始口令，不再签发 Token，不再校验口令哈希。
  `/api/meta` 去掉 `webui_enabled` / `no_auth` / `must_change_password`，
  `auth_mode` 只剩 `luci_session`（OpenWrt）与 `trusted_source`（独立部署）两种。
- **OpenWrt 无浏览器侧加密前端**：LuCI 视图不再加载 `taygedo-crypto.js`、
  不再构造 `X-TGD-Envelope` 信封、不直连应用端口；数据由 rpcd ucode 插件
  `luci.taygedo` 在服务端以已认证 LuCI 会话身份调用 `http://127.0.0.1:<port>`
  后回传给页面。视图只做展示与交互。
- **后端端口不对 LAN 暴露**：OpenWrt init 脚本固定 `TAYGEDO_LISTEN=127.0.0.1:<port>`；
  rpcd ACL 按方法粒度显式声明权限（读 / 写分离），未声明的方法一律拒绝，
  且不授予 `/etc/taygedo/**` 文件读写。
- **加密信封层的归宿**：服务端信封层与非 OpenWrt 的浏览器侧加密前端**保持现状**
  ——独立部署的外部 API 客户端仍是真实消费者；OpenWrt 侧已无消费者，
  其唯一入口（LuCI 视图）已移除加密加载路径，`static/` 资源不再随 OpenWrt 包装机。
  另：OpenWrt 上即便 `crypto_policy=always`，回环内部通道也不强制信封
  （该通道只能由本机进程触达，且叠加 LuCI 会话校验）。
- **鉴权缺陷修复**：`luci_session.rs::parse_session_get` 同时接受顶层 `username`
  与 rpcd 真机实际返回的 `{"values":{"username":"root"}}`（顶层为空时回落），
  新增回归测试覆盖顶层 / values / 两者缺失或为空三种输入。
- **配置字段清理**：`web_username`、`web_password_hash`、`web_password_salt`、
  `web_password_version`、`web_password_must_change`、`lan_no_auth`、
  `initial_password_announced` 全部移除；旧数据文件里的未知字段读取时被忽略。
- **打包与 CI 同步**：打包脚本、OpenWrt Makefile、CI 不再打包 WebUI 资源，
  也不再传递 `TAYGEDO_WEB_PASSWORD`；`scripts/e2e_web.mjs` 改为无登录、无 `Authorization`
  的协议测试。
- **启动日志**：不再打印 WebUI 访问地址或初始口令；OpenWrt 提示块改为说明
  "无内置 WebUI、无浏览器侧加密前端，数据经 rpcd/ucode 内部通道"。

### 安全 · 方案 A：鉴权边界收敛到 rpcd ACL

- **会话号只认服务端真值**：`/usr/share/rpcd/ucode/luci.taygedo` 新增 `caller_session(request)`，
  从 `request.session`（较新 rpcd 提供）或 `request.args.ubus_rpc_session`（本固件
  `rpcd-mod-ucode` 实际注入的位置）取会话号；前端传入的 `args.sid` 只用于保持 ubus 签名兼容，
  **不作为信任依据**。客户端若自己在 params 里伪造 `ubus_rpc_session`，ubus 直接以
  `Invalid parameters`（-32602）拒绝，该键无法被前端抢先写入。
- **鉴权边界 = rpcd ACL**：`acl.d/luci-app-taygedo.json` 继续按方法粒度声明 read / write
  （只读方法在 read，改配置 / 加删账号 / 签到 / 验证码在 write），未登录会话或未被授予该 ACL
  的会话调不到 `luci.taygedo` 的任何方法（实测 `-32002 Access denied`）。
- **后端回环路径不再嵌套 ubus**：`src/web.rs` 新增纯函数 `session_gate(openwrt, loopback, sid)`，
  把「运行形态 + 来源 + 会话号」压成四种结果：
  - `OpenWrt + 回环 + 会话号形态合法` → 放行（`InternalChannel`），**不再调用**
    `ubus call session get`；只校验形态（1–128 位、`[A-Za-z0-9_.:-]`）。
  - `OpenWrt + 回环 + 形态非法` → `401 luci_session_invalid`。
  - 非回环来源（含 OpenWrt 与独立部署）→ `UbusCheck`，保持原有 `ubus call session get`
    校验强度，不因修 OpenWrt 而放宽外部路径。
  - 空会话号 → `401 luci_session_missing`。
  死锁根因是 rpcd 的 ucode 方法在其自身 uloop 内同步执行，插件再同步发起 `ubus call`
  等不到同一进程应答，实测恰好耗满后端 `UBUS_TIMEOUT`（2 秒）后按 fail-closed 返回 401。
- **信任模型与代价**：回环路径的信任前提是「本机进程 + rpcd 已认证会话」；本机 root 可以直接
  `curl http://127.0.0.1:<port>` 并自带形态合法的会话号，这与 root 已有的权限等价，
  不构成权限提升。真正约束受限用户的是 rpcd ACL——受限 LuCI 用户即便打开页面，
  也调不到未授权方法。两套路径的差异与信任前提见 `docs/security-transport.md` §8.2。

### 验证

- **本地静态与测试**（实跑）：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo test --all-targets`（91 passed / 0 failed，含方案 A 的 4 个会话门单测）、
  `cargo build --release` 全部通过；改动文件 CRLF 计数为 0。
- **本地冒烟**（`TAYGEDO_EDITION=openwrt`，临时实例 `127.0.0.1:18787`）：`GET /` 返回 `404`；
  `/api/meta` 只剩 `ok/edition/auth_mode/crypto_policy/crypto_required/crypto_proto/crypto_alg/crypto_identity_kid`；
  `/api/config` 无会话 `401 luci_session_missing`、形态非法 `401 luci_session_invalid`、
  回环 + 形态合法会话号 `200`（本机无 ubus，能 200 即证明回环路径确实不再依赖 ubus），
  冷首调耗时 0.97ms；`/api/login`、`/api/logout`、`/api/password` 均为 `404`。
- **真机**（ImmortalWrt / aarch64_cortex-a53，部署分支构建产物后实测）：`parse_session_get`
  已能识别 rpcd 返回的 `{"values":{"username":"root"}}`；带 `X-TGD-Luci-Session` 直连回环
  请求 `/api/config` 返回 `200`，无会话 / 形态非法一律 `401`。
- **真机冷会话（方案 A 关键观测）**：`/etc/init.d/taygedo restart` 清空会话缓存后，
  经 LuCI 真实的 `POST /ubus`（`luci.taygedo`）首次调用 `config_get` 与 `meta`，
  均返回 `http:200`，实测耗时 **50ms**（进程内 uptime 差值计），
  不再出现此前"恰好耗满 `UBUS_TIMEOUT` 2 秒后 401"的现象；
  无会话 / 伪造会话在该路径上被 rpcd ACL 直接拒绝（`-32002 Access denied`）。
- **真机端口与来源**：`netstat` 仅见 `127.0.0.1:8787`（无 `0.0.0.0`）；
  从设备自身访问 `http://192.168.88.1:8787/api/meta` 连接失败（rc 非 0）；
  直连回环的 `/api/config` 无会话 `401`、形态非法会话号 `401`。
- **真机页面**：无头浏览器（Playwright，SSH 端口转发）登录 LuCI 后打开
  「服务 → 塔吉多签到」，页面正常渲染账号列表、运行日志与全局设置，
  控制台无 401、业务接口无 4xx/5xx（唯一的 403 是登录前 `/cgi-bin/luci` 自身的既知行为）；
  `GET /`、`/api/login`、`/api/logout`、`/api/password` 均 `404`。

### 安全 · 升级受 RustSec 公告影响的传递依赖

`v0.7.0` 的发布流水线里 `dependency-audit` 作业实测报告 **9 条漏洞**
（另有 1 条 unsound、1 条 yanked 告警）。本次只在 semver 兼容范围内做
`cargo update`，不改变任何直接依赖的版本约束、也不加 ignore 列表：

| 包 | 原版本 | 新版本 | 公告 | 修复版本 |
|---|---|---|---|---|
| `rustls` | 0.23.43 | 0.23.46 | RUSTSEC-2026-0285 / 0346 / 0348 / 0349 | >=0.23.45、>=0.23.46 |
| `rustls-webpki` | 0.103.14 | 0.103.16 | RUSTSEC-2026-0382（CVE-2026-111943） | >=0.103.16 |
| `webpki-roots` | 1.0.9 | 1.0.11 | RUSTSEC-2026-0381（CVE-2026-111943） | >=1.0.10 |
| `quinn-proto` | 0.11.17 | 0.11.19 | RUSTSEC-2026-0338 / 0340 / 0343 | >=0.11.18 |
| `tokio` | 1.53.1 | 1.54.1 | RUSTSEC-2026-0386（unsound: `AsyncFd` I/O safety） | >=1.53.3 |

其中 `rustls` 的四条正是 GitHub 默认分支上 4 个 Dependabot 告警
（2 moderate / 2 low）的同一批 GHSA；`quinn-proto` 的三条与
`rustls-webpki` / `webpki-roots` 的两条 Dependabot 尚未列出。

`quinn-proto` / `quinn` 只经 `reqwest` 的 **可选 `http3` 特性** 进入
`Cargo.lock`（本项目只用 `rustls-tls`，该特性未启用，编译产物里不含任何
`quinn*` 目标文件），但 `cargo-audit` 扫描的是锁文件，因此一并升级。

**残余告警（无法用 `cargo update` 消除，也不需要 ignore）**：`chacha20 0.10.1`
已被上游撤回（yanked），它只由 `rand 0.10` → `quinn-proto` 的 `http3` 路径
引入；本项目未启用该特性，该包不参与编译，锁文件里的这一条属于上游
依赖图噪声，等上游重新发版后随 `cargo update` 自然消失。

审计作业的策略未改动：仍是 `rustsec/audit-check@v2`、仍无 `ignore` 列表、
未调高阈值、未关闭作业。

## [0.7.0] - 2026-11-02

### 变更 · OpenWrt 上退役内置 WebUI（LuCI 唯一入口）

- **OpenWrt 目标不再提供内置 WebUI**：`GET /` 返回 `404`（`code=webui_retired`），
  不再下发首页、登录页与自助改密界面；入口统一为 LuCI 菜单「服务 → 塔吉多签到」。
  非破坏式实现——`src/ui.html` 等源码保留，仅在该目标下默认不启用。
- **登录与鉴权交给 OpenWrt**：LuCI 视图把 `L.env.sessionid`（`ubus_rpc_session`）
  放进 `X-TGD-Luci-Session` 头，服务端调用本机 `ubus call session get` 由 rpcd
  判定有效性，并要求来源命中 `lan_cidrs`；不通过分别返回 `401 luci_session_*`
  或 `403 untrusted_source`。校验结果短 TTL 缓存，超时/ubus 不可用一律 fail-closed。
- **会话校验有资源上界**：同时最多 4 个 ubus 校验在跑（拿不到许可直接按无效处理，
  不排队）；单次调用到 2s 仍未退出就 `kill()` 并回收子进程。两者都必需——
  `spawn_blocking` 无法取消、`Command::output()` 会一直等到子进程退出，只丢弃
  超时句柄会让阻塞线程与 ubus 子进程在压力下堆积。
- **应用自有登录端点在该目标下废弃**：`POST /api/login`、`POST /api/password`
  返回 `410 Gone`（`code=deprecated_endpoint`）；`POST /api/logout` 返回
  `{"ok":true,"deprecated":true}`。三个端点仅非 OpenWrt 平台可用。
- **不再生成随机初始口令**：OpenWrt 上不生成、也不使用任何应用自有口令；
  `TAYGEDO_WEB_PASSWORD` 在该目标下忽略。
- **前端资源清单按目标裁剪**：OpenWrt 上 `/static/manifest.json` 不再包含 `/`
  （首页已退役），资源数由 11 降为 10，避免 `web_guard.js` 去校验一个必然 404
  的首页而导致整体完整性校验失败。`/static/*` 仍逐文件校验。
- **平台判定可显式覆盖**：新增 `TAYGEDO_EDITION=openwrt|standalone`（未设置时
  按 `/etc/openwrt_release` 是否存在判定）；新增 `TAYGEDO_LUCI_SESSION_CHECK`
  作为排障开关（`0` 退回"仅受信内网放行"，等价旧的 `no_auth + lan_no_auth`）。
- **LuCI 前端同步调整**：移除「外部 WebUI」按钮、改密区块与「内网免鉴权放行」
  开关；鉴权失败页改为解释 LuCI 会话状态并提供重新加载；启动探测改走带会话头的
  API 路径，按错误码区分来源拒绝 / 会话缺失 / 会话失效 / 服务未运行。
- **OpenWrt 包同步**：init 脚本注入 `TAYGEDO_EDITION=openwrt` 与
  `TAYGEDO_LUCI_SESSION_CHECK`，不再传递 WebUI 口令；UCI 新增
  `luci_session_check`，并把 `web_password` / `no_auth` / `lan_no_auth` 标注为
  已废弃；包描述与 ACL 说明改为"LuCI 唯一入口、鉴权交给 OpenWrt"。

> **维护边界（已被上方 `[Unreleased]` 取代）**：本条描述的是"保留源码、仅在该目标下不启用"
> 的过渡形态；`[Unreleased]` 中内置 WebUI 已从代码库彻底删除，`src/ui.html` 不复存在。

### 验证

- `cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、
  `cargo test --all-targets` 与 `cargo build` 均通过；CI 的 quality 作业给出同样结论。
- 改动文件行尾均为 LF（CRLF 计数为 0）。
- **`ubus session get` 路径已在真机 OpenWrt 上实测通过**（ImmortalWrt SNAPSHOT，
  `aarch64_cortex-a53`，候选二进制是 CI 产出的 `aarch64-unknown-linux-musl`）。
  以 `TAYGEDO_EDITION=openwrt` 在 127.0.0.1 高位端口跑临时实例，生产二进制与配置
  全程未改动，实测：`GET /` 返回 `404 webui_retired`；`/api/meta` 为
  `edition=openwrt` / `webui_enabled=false` / `auth_mode=luci_session`；无会话访问
  `/api/config` 返回 `401 luci_session_missing`；伪造 sid 走真实 ubus 校验返回
  `401 luci_session_invalid`（7.3ms）；`POST /api/login` 返回 `410`。
- **超时截断与并发上界实测生效**：注入一个 `exec sleep 10` 的 ubus 替身让校验挂起，
  单请求在 2.003s 返回失败（三轮复测 2.003 / 2.004 / 2.004s），再过 3s 后替身
  子进程数为 0、僵尸数为 0（请求期间该子进程确为本实例的直接子进程，PPid 相符）。
  8 个不同 sid 并发时存活替身进程峰值为 4（上限生效），其中 7 个约 2.00s 返回、
  1 个 4.01s 返回，批次墙钟约 4s，未观察到进程泄漏。

## [0.6.0] - 2026-11-02

> **破坏性安全升级**：配置不再以明文落盘，WebUI 通信协议升级到 `taygedo/v2`。
> 升级前请先阅读 [docs/encrypted-storage.md](docs/encrypted-storage.md) 的「备份与恢复」一节，
> 并确认 `keys/master.key` 可与密文分开备份。
> 旧版（0.5.x）数据目录**可直接使用**，首次启动会自动迁移并保留加密备份。

### 新增 · 配置文件静态加密

- **全部受管文件密文化**：`accounts.json` / `config.json` / `state.json` / `keyring.json`
  一律以 **XChaCha20-Poly1305 + HKDF-SHA256** 密文落盘，OS CSPRNG 生成 salt/nonce。
- **统一信封格式**：Magic `TGDENV01` + 格式版本 + 算法号 + KeyID(16B) + Salt(32B) +
  Nonce(24B) + 密文长度 + 密文 + Tag(16B)；解析严格校验边界，截断/追加/零长度一律拒绝。
- **AAD 绑定文件逻辑身份**：`taygedo/storage/aad/v1` + 版本 + 算法 + KeyID + 身份，
  跨文件搬移、跨目录搬移、跨主密钥解密全部在解密前失败。
- **三平台主密钥保护**：Debian 优先 systemd `LoadCredentialEncrypted`（TPM2）、
  回落 root `0600` 文件；Windows 用 DPAPI（默认用户作用域，可用
  `TAYGEDO_DPAPI_MACHINE=1` 切机器作用域）；OpenWrt 用 root `0600` 文件，
  不依赖 systemd/TPM，不影响 procd 与 LuCI。
- **旧版明文自动迁移**：识别格式 → 按身份校验 JSON 结构 → 加密 → 临时文件 →
  回读校验 → 原子替换 → 保留**加密**备份（`.enc-bak-<ts>`，最多 3 份）→ 记录 journal。
  支持中断续做、幂等、失败不覆盖原文件、不删除明文。
- **密钥轮换**：`taygedo-rs --rotate-storage-key`，新密钥先落 `master.key.pending`，
  逐文件重加密后原子提升；中断后**一次重启即可续做完成**。
- **绝不静默丢数据**：密钥缺失/损坏、密文认证失败、权限过宽 → 启动期 `[fatal]` 退出并
  保留原文件；运行期返回 `Err`。不再有「损坏就改名 `.corrupt-*` 后返回默认配置」的行为。
- **文件系统防护**：`O_NOFOLLOW`、`fstat` 校验 `nlink==1` 与属主、权限严格 `0600`、
  `O_EXCL` 随机临时名、`fsync` 文件与目录、`fs4` 排他锁防并发实例。

### 新增 · 应用层加密协议 v2

- **协议重写为 `taygedo/v2`**：Ed25519 长期身份 + X25519 临时密钥交换 +
  HKDF-SHA256 + **XChaCha20-Poly1305**（替换 v1 的 AES-256-GCM 与计数器 nonce）。
- **握手签完整 Transcript**：版本 + 双方临时公钥 + 双方随机数 + 身份公钥 + sid + ts，
  并由服务端 Ed25519 签名；客户端验签 + 指纹比对（TOFU）后才派生 c2s/s2c。
- **请求/响应 AAD 绑定** `kid ‖ sid ‖ seq ‖ method ‖ path?query ‖ nonce`，
  响应额外绑定 `req_seq`；客户端校验 `rseq`，防响应替换与乱序错配。
- **防重放改为滑动窗口**（4096）：解决并发乱序请求与「严格递增」冲突；
  仅在 AEAD 成功后消费序号，伪造报文不能挤掉合法序号。
- **前端资源完整性**：`/static/manifest.json` 由 Ed25519 签名，覆盖 11 个资源
  （含首页 HTML）；`web_guard.js` 验签 + 指纹比对 + 逐文件 SHA-256，
  不通过则**业务 JS 不执行、登录按钮禁用**。
- **纯 JS 密码学**：内置 `@noble/ciphers`、`@noble/curves`、`@noble/hashes`
  （MIT，Cure53 审计），因为普通 HTTP 属于非安全上下文，`crypto.subtle` 不可用。
- **GET/HEAD 信封走 `X-TGD-Envelope` 头**：`fetch` 与 undici 不允许 GET 带 body；
  头与 body 走同一校验路径，两者并存直接 `400 malformed`。
- **服务端结构拆分**：`web_security/` 下新增 `error.rs`、`session.rs`、`middleware.rs`，
  加解密中间件从 `web.rs` 移出，业务 handler 只看到明文 JSON。
- **新增接口**：`GET /api/secure/identity`（身份公钥与指纹）、`GET /static/manifest.json`。

### 变更

- `Store` 对外 API 改为返回 `Result`：`load_accounts()` / `load_config()` / `load_state()` /
  `load_keyring()`。调用方必须处理错误，不再有"读失败返回默认值"的路径。
- `AppState::new` 返回 `Result`，存储层失败 → `[fatal]` 退出（不再带病启动）。
- 启动横幅新增「配置加密」与「Key ID」两行；`/api/meta` 新增 `crypto_proto` /
  `crypto_alg` / `crypto_identity_kid`。
- 删除 `src/session.rs`（v1 会话状态机），由 `web_security/` 取代。
- 传输层不再使用 AES-256-GCM（v1 的计数器 nonce 方案整体退役），因此 README 里
  关于 aarch64 `aes_armv8` 编译参数的性能/侧信道提示**不再适用于传输加密**。
  注意：`aes` / `aes-gcm` 依赖仍然保留——它们分别用于游戏接口的签名计算
  （`protocol.rs`）与账号凭据的二次加密（`crypto.rs`），与本次改造无关。
- 质量门禁抽成可复用工作流 `.github/workflows/quality.yml`（fmt / clippy
  `-D warnings` / `cargo test` / 真实二进制端到端协议测试），触发条件为
  `pull_request`(main) / `workflow_dispatch` / `workflow_call`：PR 打开或更新即可
  看到结论，`build.yml` 的 `release` 通过 `uses:` 复用同一份定义并 `needs` 它。
  抽成单一来源而不是复制两份，是因为两份脚本必然漂移，且漂移方向通常是
  "PR 上跑的那份悄悄放宽了"。
- `.github/workflows/build.yml` 新增 `audit` 作业（RustSec advisory，非阻塞）：
  一条与本次改动无关的新 advisory 不应把发布流水线卡死；结论在 Actions 里
  显式可见，由维护者评估后单独升级依赖。
- 原有 Windows / musl x86_64 / musl aarch64 / debian / openwrt(ipk+apk) /
  release 流程保持不变。
- `scripts/package.sh`：deb 的 systemd unit 补充 systemd 凭据用法说明，postinst
  收紧 `/var/lib/taygedo` 与 `keys/` 权限；OpenWrt `init.d` 兜底 `chmod 700/600`。
- README 的加密章节重写，明确区分「传输加密」与「存储加密」，并如实列出
  本次已验证与**尚未在真机验证**的项目。

### 修复

- **续做轮换后仍用旧密钥**：`resume_rotation` 会把 `master.key.pending` 提升为正式密钥，
  但进程内 `StorageCrypto` 仍持旧密钥，导致本次启动读新密文必然 `KeyIdMismatch`
  ——等于要求用户重启两次。现在续做成功后立即重新装载主密钥。
  （新增测试 `interrupted_rotation_is_resumed_in_one_start`，去掉修复即失败。）
- **CI 首次运行拦下的两处 lint**（本地 rustc 1.93 不报，CI 用 1.99 才触发）：
  - `clippy::for_kv_map`（`src/protocol.rs`）：遍历 `BTreeMap` 却丢弃 key，
    改为 `data.values()`。语义等价——`BTreeMap` 本就按 key 字典序迭代。
  - `clippy::result_large_err`（`src/web_security/middleware.rs`）：中间件返回
    `Result<Response, Response>`，`Err` 变体 128 字节。这是 axum 的既定约定
    （`Err` 必须实现 `IntoResponse`），装箱只会多一次堆分配、不减少实际成本，
    因此加 `#[allow]` 并写明理由，而不是为了消 lint 去改签名。
- **`crypto_policy=never` 下 WebUI 不可用**（Codex 评审 P2）：WebUI 此前无条件
  发加密信封，而 `never` 策略下中间件不解密，登录等 POST 会让业务 handler
  收到密文 JSON 而失败。`src/ui.html` 现在先读 `/api/meta` 的 `crypto_policy`，
  仅在 `never` 时改走明文请求，其余策略保持信封路径（不回退明文）。
- **非 2xx 响应对调用方不可读**（Codex 评审 P2）：业务错误（如 401）同样被
  中间件加密，但 `static/taygedo-crypto.js` 在 `!r.ok` 分支直接返回密文，
  调用方丢失真实错误；同时明文 `no_session` 被当成普通结果返回，
  重握手逻辑永远不触发。现在按 `X-TGD-Enc` 标记（缺失时按信封结构）统一解密，
  明文 `crypto_error` 则抛错进入「重建会话 + 重试一次」。
- **LuCI 跨源预检缺少 `X-TGD-Envelope`**（Codex 评审 P2）：同主机不同端口时
  GET/HEAD 信封走该请求头，而 `cors_middleware` 的
  `Access-Control-Allow-Headers` 未声明它，浏览器会在预检阶段拦下请求。
  现统一为常量 `ALLOWED_REQUEST_HEADERS`（预检响应与常规响应共用一处定义）。

### 测试

- 单元/集成测试 **97 passed / 0 failed**（Linux，`cargo test --all-targets`）。
  Windows 本机为 **85 passed / 0 failed**：8 个既有 Unix 专用用例 + 本次新增
  `#[cfg(unix)]` 门禁的 4 个用例（断言 `keys/master.key` 布局、密钥篡改、
  轮换、续做轮换）在 Windows 上不适用，如实跳过而非改断言凑数。
- 端到端协议测试 `scripts/e2e_web.mjs`：真实二进制 + 仓库内真实前端文件，
  **26/26 通过**。覆盖：
  - 清单验签 + 11 个资源哈希；
  - 加载真实的 `web_guard.js` 对真实清单执行完整性校验：TOFU 首次、指纹已固定、
    指纹变更被拒、资源被替换（哈希不符）被拒、资源恢复后自愈；
  - 跨源预检声明 `X-TGD-Envelope`（对应本次 `cors_middleware` 修复）；
  - 握手验签、登录、账号/配置/日志往返；
  - 重放拒绝、跨接口搬移拒绝、GET 走头与 POST 走 body、正文与头并存拒绝、明文 428；
  - 错误登录返回 401 且响应已解密（调用方拿到 `error` 而非密文）；
  - 伪造会话 ID 触发 `no_session` 后自动重握手并成功（会话 ID 已更新）。
- `cargo fmt --all -- --check` 与 `cargo clippy --all-targets -- -D warnings` 均无输出。
- 上述 fmt / clippy / 测试在 **rustc 1.99.1**（与 CI 同版本）下复验通过。此前本地
  为 1.93，有两条新 lint 未触发，已由 PR 门禁拦下并修正——这也是把质量门禁放到
  PR 上的直接理由：本地版本落后时，"我这儿是干净的"并不等于 CI 干净。
- `scripts/e2e_web.mjs` 的 vendor 模块动态 import 改为 `pathToFileURL`：
  原先传裸文件路径，Windows 下被 ESM loader 判为未知 scheme（`Received protocol 'c:'`），
  该缺陷只在非 Linux 本地复跑时暴露，CI 不受影响。

## [0.5.2] - 2026-11-01

### 新增 & 优化 · 视觉与交互（白色 Material Design 3 全面重构）

- **LuCI 界面与独立 WebUI 统一为白色 MD3 风格**：
  - 基于 Material Design 3 规范重新设计色彩系统与设计 Token（`--md-primary: #3b66f5`、`--md-surface: #ffffff`、微透明毛玻璃卡片 `rgba(255, 255, 255, 0.88)` + `backdrop-filter: blur(20px)`），整体视觉轻盈、质感清透。
  - **背景壁纸无缝保留**：完整保持原精美背景壁纸（`uu4H/bg1.jpg`），叠加轻柔的白昼毛玻璃遮罩层，图文对比更柔和清晰。
  - **全新默认头像**：默认头像替换为高清粉发角色头像（PNG 格式及 Base64 Data URI 直出），零外链即开即显。
  - **组件 MD3 规格重构**：数据统计区升级为彩标卡片与平滑计数值；账号卡片升级为 20px 大圆角立体卡片与状态胶囊徽章（Chip）；模态弹窗与操作按钮全面重绘为 MD3 悬浮阴影与平滑过渡形态。

### 优化 · 移动端与 PC 双端响应式体验

- **深度适配手机与大屏 PC**：
  - PC 端采用宽屏流式双列网格，大屏幕视觉开阔均衡；
  - 移动端自适应折叠为单列流式堆叠布局，支持 `viewport-fit=cover` 适配刘海屏及底部安全区；
  - 针对触屏操作重构各表单输入项与操作按钮热区尺寸，彻底消除误触并改善移动浏览器滑动阻尼感。

### 优化 · 架构与部署

- **WebUI 支持热更新**：Axum WebUI 入口增加对 `/etc/taygedo/ui.html` 与 `/www/taygedo/ui.html` 本地覆盖文件的探测，有覆盖时优先加载，无覆盖时自动回退为内嵌静态资源，无需重编译二进制即可热替换自定义前端。
- **构建环境兼容性加固**：修正 `.cargo/config.toml` 中 `include` 字段格式为数组类型，规避新版 Cargo 解析警告。

## [0.5.1] - 2026-09-30

### 修复 · CI 编译配置（避免下游编译失败）

- **`.cargo/config.toml` 不再被整个删除**：此前 CI 为绕过 rsproxy 镜像在
  Linux runner 上执行 `rm -f .cargo/config.toml`，把 aarch64 的
  `aes_armv8` rustflags 一并删掉——aarch64 musl 交叉编译因此静默退化为
  软件 AES 后端（性能差约一个数量级 + cache-timing 侧信道），直接抵消
  0.5.0 的硬件加速加固。现改为「镜像配置独立成文件」：
  - 新增 `.cargo/config.mirror.toml`（源替换 / rsproxy 镜像 / git-fetch 配置），
    由 `.cargo/config.toml` 以 `include` 引入；
  - CI 只把 `config.mirror.toml` 覆盖为空文件以切回官方 crates.io，
    **完整保留** `.cargo/config.toml` 中的 target 级 rustflags；
  - 该步骤不再限定 Linux runner，Windows / Debian / musl 各作业行为一致。
- **OpenWrt Makefile 架构映射与发布产物对齐**：`luci-app-taygedo` 的预编译
  下载路径原先仍为 `arm` / `mipsel` / `mips` 等架构映射 musl target，而 CI
  实际只发布 x86_64 与 aarch64 两个架构，导致这些架构在 `Build/Prepare`
  下载阶段必然 404，报错晦涩（曾在 `arm_cortex-a7` 等常见架构上造成下游
  编译失败）。现仅保留 x86_64 / aarch64 映射，其余架构 `$(error)` 快速失败
  并提示改用「方式 B：源码编译」，不再静默下载错误架构的产物。
- **OpenWrt 包 Makefile 三处兼容性修复**（`luci-app-taygedo`）：
  - 架构判断由 `$(CONFIG_ARCH)` 改为 `$(ARCH)`：OpenWrt 包上下文里
    `CONFIG_ARCH` 未定义/为空，导致所有 ifeq 分支落空、静默下载
    x86_64 二进制，aarch64 设备装进后无法运行；
  - `Build/Prepare` 下载由 `download.pl` 改为 `curl -fsSL --retry 3`：
    原先传参不足（`<file> <url>` 而脚本要求 `<dir> <file> <hash> <url>...`），
    缺参直接打印 Syntax 并以 255 退出；
  - `Build/Compile` 由 `$(TARGET_STRIP)` 改为 `chmod 755`：预编译产物是
    aarch64 静态 musl 二进制（Release 资产已 stripped），在 x86_64 宿主上
    执行 TARGET_STRIP（展开为空时退化成直接运行该二进制）会报
    `cannot execute binary file` / Error 126。
  这三处修复使下游 OpenWRT-CI 不再需要对该 Makefile 打 sed 补丁。

### 优化 · CI 编译

- **原生构建启用 cargo 缓存**（`Swatinem/rust-cache`）：Windows 与 Debian
  作业按 `Cargo.lock` 缓存 registry 与 target 目录，重复构建显著提速；
  musl `cross` 构建依赖 Docker 层缓存，维持现状。
- 版本号同步至 `v0.5.1`：`Cargo.toml` / `Cargo.lock` / `Makefile` 的
  `PKG_VERSION` 一致更新，预编译产物下载指向即将发布的 tag。

### 修复 · 解析期架构 $(error) 导致下游包未被 .config 选中

- **问题**：`Makefile` 顶层对不支持架构使用 `else $(error ...)`，而 OpenWrt
  在 `make defconfig` 生成 `tmp/.config-package.in` 的元数据扫描阶段会 include
  本 Makefile，此时 `$(ARCH)` 尚未从 target 解析出来（为空）。空 `$(ARCH)`
  触发解析期 `$(error)`，该包被 packageinfo 扫描丢弃，`CONFIG_PACKAGE_
  luci-app-taygedo` 变成未知符号并被 defconfig 清除，最终 .config 中无本包，
  表现为「luci-app-taygedo 未选中」，导致下游 OpenWRT-CI 的 H5000M 自用配置
  编译失败且难以定位。
- **修复**：架构映射只做「已知架构 → triple」的正向 `ifeq`，未知/空架构令
  `TAYGEDO_TRIPLE` 为空（解析期永不出错）；真正的 fail-fast 移到 `Build/Prepare`
  的 recipe 头部——若 triple 为空则以明确报错退出。这样扫描期包能正常注册，
  构建期对不支持架构依旧 fail-fast 并给出源码编译指引。
- **验证**：解析期空 `ARCH` 不再中止；`ARCH=aarch64`（H5000M）可正常下载；
  不支持架构在构建期按预期报错。

## [0.5.0] - 2026-09-19

本版本是一次**安全加固 + 应用层加密**的大版本。起因是一次针对本项目的
前后端鉴权审计：审计确认了 1 项高危（登录限速为死代码 + 初始口令固定为
`admin/admin`）、3 项中危与 3 项低危问题，并在修复完成后叠加了一套端到端
应用层加密，使得被动抓包无法还原任何请求/响应正文。

### 安全修复（来自本次审计）

- **P0 · 登录限速形同虚设（死代码）**：`login_lock_remaining()` /
  `record_login_fail()` 早已实现，但 `login_api` **从未调用**，等于完全没有
  暴力破解防护；叠加历史默认口令为固定 `admin/admin`，公网部署可被直接
  接管。现已在 `login_api` 入口处调用限速检查（触发时返回 429 并给出
  `retry_after`），失败路径记录计数，成功路径清零。**同时改为首启动生成
  随机初始口令**（见下「初始口令」）。
- **P1 · 改密后旧会话仍然有效**：原实现只更新哈希，不吊销任何 token，
  「口令已泄露→改密」这一最常用的止损动作**不生效**。现 `set_credentials()`
  在写盘后立即吊销全部登录会话与全部加密会话（返回吊销计数并记日志）。
- **P1 · 口令哈希强度不足**：原为单轮 SHA-256 + 8 字节 salt，GPU 下可高速
  爆破。现升级为 **scrypt**（N=16384, r=8, p=1, dkLen=32），并带 `web_password_version`
  版本号：历史 v1 哈希在**下次成功登录时就地升级**为 v2，用户无感知。
  比较改为恒定时间实现，避免时序侧信道枚举有效用户名。
- **P1 · CORS 允许任意来源**：原 `Access-Control-Allow-Origin: *`。现改为
  同源白名单（同 host 不同端口自动放行，覆盖 LuCI 从 80 访问 8787 的场景），
  另可用 `TAYGEDO_ALLOWED_ORIGINS` 追加显式来源。
- **P2 · 明文口令进日志**：首启动把明文口令写入日志缓冲区，而 `/api/logs`
  可读该缓冲区，形成二次泄露。现明文**只**在启动横幅（stdout）出现一次，
  日志内仅记录「已生成随机口令，请在启动横幅查看」。
- **P2 · 改密接口信任请求体身份**：`change_password` 原从请求体读 `username`
  作为操作主体，理论上可被用于针对任意账号。现操作主体一律从**服务端会话**
  （`AuthIdentity`）解析，完全忽略请求体中的身份声明；新增的 `new_username`
  语义是「要改成的新账号名」，而非身份声明。
- **P2 · 未鉴权端点信息暴露**：`/api/meta` 收窄为仅返回模式标志
  （`crypto_policy` / `crypto_required` / `no_auth` / `must_change_password`），
  不含任何凭据、路径或密钥信息；移除了从无写入方的 Cookie 鉴权分支
  （死代码 + 一旦启用即引入 CSRF 面），鉴权收敛为 Bearer 单一通道。
- **初始口令**：`admin/admin` 不再存在。首次启动生成高强度随机口令，
  仅在 stdout 横幅出现一次，并通过 `must_change_password` 标志在 UI 上
  提示用户立即修改；改密后该标志自动清除。

### 新增 · 应用层端到端加密

在 HTTP 之上（而非替代 HTTP）实现了一套会话级 AEAD 通道，目标是
**被动抓包无法还原请求与响应正文**（含明文口令、Cookie/Token）。

- **算法与密钥编排**：X25519 密钥交换 → HKDF-SHA256 域分离 → AES-256-GCM。
  每次握手服务端使用**临时密钥对**，因此具备**前向保密**：长期身份私钥
  （`keyring.json`）只用于客户端固化校验（抵御中间人），**不参与**对称密钥
  派生，泄露它无法解密历史流量。
- **域分离**：`taygedo/v1/aead` → 主密钥，再分出方向独立的 `c2s` / `s2c`
  子密钥，以及各自的 4 字节 nonce 前缀；双向密钥与 nonce 空间互不重叠。
- **Nonce 管理**：`4 字节 HKDF 派生前缀 || 8 字节大端计数器`，同一密钥下
  **全局唯一**，从构造上排除随机 nonce 的碰撞风险。
- **AAD 绑定**：`sid || 方向(1B) || 序号(8B)`（响应额外绑定请求序号），
  把密文与其会话/方向/位置绑定，防止跨会话、跨请求搬运密文。
- **防重放**：每方向单调递增序号，接收侧只接受 `seq > last_seq`，且
  **仅在认证标签校验通过后**才推进序号（失败报文不消耗序号，避免被用来
  提前耗尽窗口实施 DoS）。
- **握手抗重放**：HKDF salt 为 `client_pub || server_pub || client_nonce || server_nonce`，
  四个字段全部参与，使重复握手必然导出不同密钥。
- **低阶点拒绝**：共享密钥为全零（低阶点攻击）时直接拒绝，不进入派生。
- **密钥材料清零**：共享密钥、主密钥、子密钥中间量用 `zeroize` 显式擦除，
  会话结构体实现 `Drop` 时清零；长期私钥与签发 token 均不落盘明文日志。
- **握手限速**：每来源 60 秒内最多 60 次握手，独立于登录限速。
- **错误不泄露 oracle**：解密失败对外统一映射为粗粒度错误码
  （`auth_failed` / `malformed` / `replay` / `no_session`），不区分
  「填充错误」与「标签错误」，避免 Padding Oracle 类攻击。

### 新增 · 加密策略与内网豁免

`crypto_policy` 三档，可按部署环境选择，兼顾安全与资源受限设备：

| 值 | 行为 | 适用 |
| --- | --- | --- |
| `auto`（默认） | 内网明文直通，非内网来源强制加密 | OpenWrt / 内网自用（CPU 开销最低） |
| `always` | 所有来源强制加密，明文请求返回 428 | 公网暴露 / 严格合规 |
| `never` | 关闭应用层加密 | 排障、兼容旧客户端 |

- `lan_cidrs`：内网判定白名单（默认 RFC1918 三段 + loopback + IPv6 ULA/链路
  本地），同时用于「免鉴权放行」与「auto 策略豁免」两处判定；IPv4-mapped
  IPv6（`::ffff:192.168.x.x`）会归一为 IPv4 再比对。配置写入路径使用**严格
  解析**（非法网段整次拒绝），避免静默降级为空集合。
- `lan_no_auth`：即使 `no_auth=1`，非内网来源也不放行——防止「免鉴权开关
  误开 + 服务暴露公网」演变为完全开放。

### 兼容性（本次改动的硬约束，已逐项验证）

- **OpenWrt 免鉴权模式**：不受影响。LuCI 页面走内网，`auto` 下明文直通。
- **内网免鉴权放行**：按 `lan_cidrs` 判定，语义与行为向后兼容。
- **LuCI 自身登录鉴权与权限链**：`luci-app-taygedo` 的 ACL 未被放宽；仅授予
  UCI 读写与 service 重启，**刻意不授予 `/etc/taygedo/**` 的文件访问**
  （该目录含账号与长期身份私钥）。
- **RPC/API 调用、静态资源、文件上传**：路由未变；加密为**可选叠加层**，
  客户端可继续走明文（`never` / 内网 `auto`）；请求体上限 34 MiB 覆盖文件上传。
- **升级后配置保留**：`config.json` 与 `keyring.json` 位于数据目录，opkg/apk
  不会删除运行期文件；`/etc/config/taygedo` 声明为 conffiles，升级时保留用户
  修改。历史 v1 口令哈希自动升级，旧客户端无需改动。

### 变更

- WebUI 与 LuCI 页面均新增：加密策略选择、内网网段配置、内网免鉴权开关；
  改密区域改为「新账号名 + 当前密码 + 新密码 + 确认新密码」，前端做长度
  （≥8）与一致性前置校验，成功后提示「所有会话已失效，请重新登录」。
- `scripts/package.sh` 与 `.github/workflows/build.yml` 未改；`PKG_VERSION`
  提升至 `0.5.0`。
- 新增单元测试 13 项，覆盖：握手往返、密文篡改拒绝、重放拒绝、跨会话序号
  拒绝、错误对端密钥失败、低阶点拒绝、会话全清、双向 nonce 前缀不同、
  握手限速生效、scrypt 确定性与 salt 敏感性、恒定时间比较。

### 密钥管理与运维

- **生成**：首启动自动生成 `keyring.json`（含 X25519 长期身份私钥）。
- **存储**：Unix 下强制 `0600`（父目录 `0700`）；启动时检查权限并告警；
  文件损坏时**不覆盖**，改用内存临时密钥继续运行，避免"悄悄重建"导致已
  固化客户端指纹的部署失联。
- **备份**：`keyring.json` 需纳入备份；丢失后客户端需重新固化身份指纹
  （不影响历史流量保密性，因为对称密钥由临时密钥派生）。
- **禁止硬编码**：项目中不存在任何硬编码密钥；私钥与 token 不写入源码、
  不写入日志。
- **轮换**：提供 `rotate_identity_key()`（供显式操作调用，每次递增 generation）。
- **升级保留**：数据目录不随包升级被清理，`config.json` / `accounts.json` /
  `state.json` / `keyring.json` 均保留。

### 文档

- **README 新增「应用层加密」整章**：说明威胁模型（被动抓包在内、终端被控/服务端沦陷不在内）、
  密码学方案逐项参数表、`crypto_policy` 三档语义与适用场景、`lan_cidrs` 默认值与
  「emptied 回落默认」的行为约定、免鉴权×加密的组合矩阵、密钥管理（生成/存储/备份/轮换/
  升级保留）、**改动文件与配置项清单**、向后兼容与三步回滚、以及 14 项验证清单（含实测结果列）。
- **新增原理图 `docs/images/crypto-architecture.svg`**：直观展示加密边界所在位置（前端与后端
  之间，而非 TLS）、握手阶段的密钥协商过程，以及双向密钥的域分离派生关系。
  README 在章节开头引用该图，并把「加密位于前端与后端之间」作为首句结论前置。
- **README 把加密提到产品定位层**：导语与「三步上手」均点明端到端加密与默认开启的加密策略，
  使读者在了解功能前先知道"抓包看不到内容"这一特性。
- **README 同步修订既有描述**：功能特性的「登录鉴权」由 `sha256 加盐` 更正为
  `scrypt + 常量时间比对`（并说明 v1 自动升级）；「API 一览」补充
  `/api/crypto/handshake`、`/api/logout`、`/api/meta` 三条，并声明 **0.5.0 起不再接受
  Cookie 传递 token**（仅认 `Authorization` 头）；「数据与兼容性」补充 `keyring.json`
  的定位与备份要求；目录结构补充 `session.rs` 与 `static/taygedo-crypto.js`；
  「常见问题」新增 4 条（抓包乱码是否正常、身份变化提示、局域网强制加密、OpenWrt 升级保留项）。
- **更正全部遗留的旧初始口令描述**：Windows / Debian / WebUI / 常见问题等 6 处原写
  "用 `admin / admin` 登录"，与 0.5.0 的随机初始口令设计矛盾，已统一改为
  "首次启动生成的随机初始口令，仅打印一次"，并补充忘记密码时的重置路径。
- **补齐环境变量表中的默认口令残留**：`TAYGEDO_WEB_PASSWORD` 原标注默认值为 `admin`，
  与"随机生成、无固定默认口令"的实际行为矛盾。现更正为"默认无（不设置则随机生成）"，
  并说明该变量**仅在首次初始化时生效**、落地后修改无效。

### 文档可视化

- **README 新增 7 张架构与流程图形**，使抽象说明可一眼读懂，全部存放于 `docs/images/`：

  | 图形 | 位置 | 表达内容 |
  | :--- | :--- | :--- |
  | `install-path.svg` | 三步上手 | 按运行环境选择安装方式与包格式 |
  | `architecture.svg` | 功能特性 | 界面层 / 执行层 / 数据层的解耦关系 |
  | `signin-flow.svg` | 通用使用指南 | 定时触发到写回状态的完整链路 |
  | `key-derivation.svg` | 应用层加密 | 握手与四路密钥材料的域分离派生 |
  | `crypto-policy.svg` | 应用层加密 | 加密策略的实际判定过程 |
  | `data-files.svg` | 数据与兼容性 | 四个数据文件的职责与备份要求 |
  | `auth-layers.svg` | API 一览 | 加密闸门与鉴权闸门的先后与分工 |
  | `source-map.svg` | 目录结构 | 源码按职责分为五组 |

- **原理图 `crypto-architecture.svg` 重制**：原图宽 880、固定白底，在 GitHub 深色主题下
  背景刺眼且缩放后文字偏小。现统一为 680 宽、透明背景，并通过媒体查询适配深色模式；
  同时补充"加密边界位于 HTTP 之内而非 TLS"的显式标注。

- **图形实现约定**：所有 SVG 均为自包含（无外部引用）、样式内联、不使用 `foreignObject`，
  以兼容 GitHub 的 SVG 渲染限制；深浅色两套配色均已通过计算样式检测，确认文字与背景
  对比度正常。

### 仓库元信息

- **仓库简介（About）补充加密特性**：由原来仅强调"单文件开箱即用 + 响应式 WebUI"，
  改为在显要位置点明 `X25519 + AES-256-GCM 应用层端到端加密`。
- **Topics 扩充**：由 10 个增至 18 个，新增 `encryption`、`aes-gcm`、`x25519`、
  `end-to-end-encryption`、`security`、`axum`、`musl`、`arm64`，使加密与实现栈可被检索到。
- **Homepage 指向最新 Release**，便于从仓库首页直达下载。

### 实测验证与后续修复

本版本的加密层在云服务器（aarch64-musl 交叉编译）与真实 OpenWrt 设备
（ImmortalWrt SNAPSHOT / mediatek filogic / aarch64_cortex-a53）上完成了
端到端验证，过程中定位并修复了两个只在真实环境才暴露的问题。

- **修复 · 切换加密策略会销毁发起请求自身的会话（"配置已生效但报错"）**：
  `/api/config` 修改 `crypto_policy` 时原实现调用 `destroy_all()` 作废全部
  加密会话以强制重新握手，其中**包含发起本次请求的会话**。后果是配置
  **已经成功写盘生效**，但响应无法用已销毁的会话加密返回，客户端只能拿到
  428 / 无会话错误——表现为"操作成功却提示失败"，且用户无法直接感知配置
  其实已经生效。实机复现路径：`always` 策略下经加密通道请求改回 `auto`。
  修复两处：
  1. `CryptoManager::destroy_all_except(keep)` —— 作废除指定会话外的全部
     会话，保留值为 `0` 表示无需变更；
  2. 新增 `CryptoCtx` 可选提取器（`Rejection = Infallible`）——明文通道下
     返回 `None` 而非报错，使 handler 无需区分当前是否走加密通道即可安全
     取得会话号；`update_config` 据此在策略变更时优先保留自己的会话。
  新增 2 项回归测试：`destroy_all_except_keeps_current_session`、
  `destroy_all_except_with_unknown_sid_clears_everything`。

- **修复 · aarch64 上 AES 落到软件实现（性能与侧信道风险）**：`aes` crate
  的 `aes_armv8` 是**编译期 cfg 门控**，未显式开启时 aarch64 目标会落到
  `soft` 后端——性能相差约一个数量级，且软件查表实现在存在 cache-timing
  侧信道风险。实机确认目标 CPU（Cortex-A53）具备 `aes` / `pmull` / `sha2`
  扩展，且工具链支持相应内联指令。现于 `.cargo/config.toml` 为
  `aarch64-unknown-linux-musl` 与 `aarch64-unknown-linux-gnu` 显式加入
  `rustflags = ["--cfg", "aes_armv8"]`；该配置走运行时 CPU 特性探测
  （`armv8 + autodetect`），在不支持扩展的旧 CPU 上自动回落，不会崩溃。
  注意：`[target.<triple>.rustflags]` 会**覆盖**环境变量 `RUSTFLAGS` 而非追加，
  交叉编译时勿同时依赖两者。

- **验证结论（实机）**：
  - 被动抓包对照实验：对照组（`never`）口令与 Token 在流中**各可见 1 次**；
    实验组（`always`）口令、Token、`username` 字样**均为 0 次**——核心目标达成。
  - E2E 协议客户端对真实设备 **17/17 通过**（含握手、密文篡改拒绝、重放拒绝、
    跨会话序号拒绝、错误对端密钥失败、低阶点拒绝、限速生效）。
  - 升级实测：`keyring.json` 自动生成且权限 `0600`、数据目录 `0700`；
    `accounts.json` / `state.json` md5 升级前后**完全一致**（升级无损）。
  - 历史 v1 口令哈希**就地升级为 scrypt v2**（`web_password_version` 1→2，
    salt 8→16 字节），用户以原口令无感知登录。
  - `lan_cidrs` 运行时热更新生效：白名单收窄后路由器自身地址收到 428 +
    `x-tgd-enc: 0`，`127.0.0.1` 仍 200；恢复后回到 200。
  - LuCI 独立免鉴权链路：内网 + `no_auth=1` + 无 token 下，列账号、读配置、
    读日志、改签到时间全部 200 且写入生效。
  - 本地单测 15/15 通过；aarch64-musl 交叉编译 0 代码警告。

### 回滚方案

1. 停止服务：`/etc/init.d/taygedo stop`。
2. 还原二进制与前端：恢复备份的 `/usr/bin/taygedo-rs`、LuCI `status.js`、
   `/etc/init.d/taygedo`、`/etc/config/taygedo`。
3. 数据兼容：新版仅在 `config.json` **新增**字段（`crypto_policy` 等），
   旧版读取时会忽略未知字段，**无需回退数据文件**；`web_password_version`
   为 v2（scrypt）时旧版无法校验——如需完全回退到旧版登录，须用旧版重新
   设置一次口令（或用备份的 `config.json` 覆盖）。
4. 启动：`/etc/init.d/taygedo start`。

## [0.4.12] - 2026-09-06

### 修复

- **OpenWrt 安装后 LuCI 页面不可用（实机复现）**：UCI 默认配置 `enabled '0'` 且 `opkg` / `apk` 安装时**不会**自动 enable `/etc/init.d` 下的脚本，导致装完包后服务既未启动也未注册开机自启，LuCI 页面对 8787 端口 API 的 fetch 全部失败，表现为「无法使用」。三处修复：
  1. 默认配置改为 `enabled '1'`（本包定位即 LuCI 集成服务，`no_auth` 默认 1 与之配套）；
  2. `scripts/package.sh` 的 ipk 分支新增 `postinst`（opkg 在 control.tar.gz 中执行）、apk 分支新增 `--script post-install`（APKv3 metadata 携带）：安装时自动 `/etc/init.d/taygedo enable`，并在 `taygedo.main.enabled != 0` 时立即 `start`——用户显式停用的场景不受影响；
  3. LuCI 页面探测失败时区分两种情况：fetch 连接失败（TypeError）= 服务未运行 → 新增「服务未运行」引导页（含重试按钮与 SSH 启用命令）；HTTP 非 2xx（如未开免鉴权被拒）→ 维持原「免鉴权」引导页，不再误导用户去改 `no_auth`。

### 变更

- 新增 `.gitattributes` 强制 LF 行尾：`package.sh` 在 CI/OpenWrt 上运行，CRLF 会导致 shebang 失效与 apk 脚本解析失败，从源头杜绝 Windows checkout 造成的行尾问题。
- `openwrt/luci-app-taygedo/Makefile` 的 `PKG_VERSION` 从 `0.4.9` 对齐到 `0.4.11`（预编译二进制下载指向当前已发布 tag）。

## [0.4.11] - 2026-08-31

### 修复
- **APP 签到恒失败（`appSignin：invalid request`，HTTP 200 / code=22）**：`app_signin` 此前手工拼装了一套残缺的 native 请求头（`appversion: 1.1.0`，缺 `ds`、`platform`、`Accept`）。实测确认本端点**强制要求 `ds` 签名头**——缺失时上游一律返回 `code=22 invalid request`，而补齐 `ds` 后立即返回 `code=0`；`appversion` / `platform` / `Accept` 均非必需项。现已与同一端点的 `bbs_signin`（`communityId=2`）统一复用 `native_common_headers`，不再手工构造。该失败会导致整轮签到判定为 `failed`。
- **会话续期接口同源失效（`refreshToken` 缺 `ds` → code=22，阻断 laohuToken 兜底）**：`refresh_token` 同样手工拼装残缺头（缺 `ds`），上游恒返回 `code=22 invalid request`。该错误文案不含 `REFRESH_REJECTED_402`，导致 `refresh_or_rebuild_session` 将其误判为「非 402」而跳过 `laohuToken` 重建、直接失败。补齐 `ds` 后，refreshToken 失效时上游正确返回 `402`，进而落到 `laohuToken` 重建路径（已实测 `user_center_login` 可用）。手工头中的 `appversion: 1.1.0` 一并改为 `native_common_headers` 的 `1.2.5`。
- **会话过期导致部分端点被网关直接拒签（HTTP 401）**：原逻辑「本地已有 `accessToken` 就直接使用」，而 `accessToken` 会过期；过期后网关层（istio-envoy）对 `getUserTasks`、`/apihub/awapi/*`、`/bbs/api/post/*` 等端点直接返回 HTTP 401（空响应体、1ms 内返回，未达后端），而 `code=22` 这类业务错误又不会命中 `is_auth_error`，导致当天签到永久失败且无自愈路径。现改为签到前**主动续期**（`refreshToken` 代价最小、不触发风控），续期失败时回退复用现有会话并由后续重试兜底，避免弱网下误伤。
- **金币任务帖子列表解析失败（浏览 / 点赞 / 分享恒为 0）**：`get_recommend_post_list` 原仅识别 `data.list` 或 `data` 为数组，而上游实际返回 `data` 为对象、帖子位于 `data.posts`（`{"code":0,"data":{"hasMore":true,"page":2,"posts":[...]}}`），导致解析恒为 `None`，日志报「获取帖子列表失败：…（HTTP 200，code=0，msg=ok）」这一自相矛盾的错误，金币任务中除 BBS 签到外的浏览 / 点赞 / 分享子任务全部无法执行。现按 `data.list` → `data.posts` → `data` 顺序依次尝试，兼容多种响应形态（已实测帖子的 `postId` 与 `selfOperation.liked` 字段与现有取值逻辑匹配）。

### 变更
- **APP 签到失败不再中断整轮**：原实现用 `?` 传播 `app_signin` 的错误，导致这一独立子任务失败后，游戏签到、金币任务、云时长全部不执行（故障放大器）。现改为降级记录并继续执行；`app_signin` 字段在无结果时为 `null`，失败原因合入 `error` 字段。状态判定：全部游戏失败，或 APP 签到失败且无任何游戏成功时，才判为 `failed`。
- **版本号同步**：bump 至 `v0.4.11`，`Cargo.toml` / `Cargo.lock` 同步更新。

### 构建 / 发布

- **修复手动触发（`workflow_dispatch`）时 Debian 与 OpenWrt 打包必失败**：打包步骤用 `${GITHUB_REF_NAME#v}` 取版本号，而该变量**仅在 tag 推送时才是版本号**，手动触发时等于分支名（如 `main`）。结果：
  - `dpkg-deb` 报 `'Version' field value 'main': version number does not start with digit`，`debian-amd64` 作业失败；
  - `apk mkpkg` 报 `info field 'version' has invalid value`，`openwrt-*` 作业以退出码 99 失败；
  - `ipk` 因 opkg 校验宽松而**静默产出废包** `luci-app-taygedo_main-1_<arch>.ipk`，是三者中最隐蔽的一个。
  修复分两层：
  1. **工作流层**：`debian` 与 `openwrt` 作业各新增 `Resolve version` 步骤——`GITHUB_REF_NAME` 匹配 `^[0-9]+\.[0-9]+\.[0-9]+` 时才当作版本号，否则回退为 `Cargo.toml` 的 `version`，保证任何触发方式都能产出合法版本。
  2. **脚本层（防御性兜底）**：`scripts/package.sh` 新增 `normalize_version`——去 `v` 前缀、剔除包管理器非法字符、结果不以数字开头时回退读 `Cargo.toml`，仍失败则**明确报错退出而非产出废包**。
- **打包脚本支持 release 号并按包管理器分别组合版本**：`package.sh` 新增第 6 个可选参数 `release`（缺省 1）。三种格式的合法版本写法不同，此前硬编码 `-1` / `-r1`：现为 `deb → <ver>`、`ipk → <ver>-<rel>`、`apk → <ver>-r<rel>`。tag 构建（release=1）的产物命名与此前完全一致，无破坏性变更。
- **Release 发布策略重做（双轨：正式版 tag + 滚动 nightly）**：`release` 作业此前无 `tag_name`，完全依赖 `softprops/action-gh-release` 的默认行为取 `github.ref`——tag 推送时正确，手动触发（`workflow_dispatch`）时 `github.ref` 是 `refs/heads/main`，会创建名为 `main` 的脏 Release。此前本喵以 `if: startsWith(github.ref, 'refs/tags/v')` 门控规避，但副作用是**手动构建编译完成却不上传 Release**。现改为按触发方式分流：
  - **tag 推送 `vX.Y.Z`** → 正式版：`tag_name=vX.Y.Z`，标题 `Taygedo vX.Y.Z`，非预发布；
  - **手动触发** → 滚动 nightly：固定 `tag_name=nightly`，标题 `Taygedo Nightly v<版本> (build <构建号>)`，标记 `prerelease=true`，每次覆盖更新，tag 数量恒定不膨胀。
  - nightly 发布前先清空该 Release 的全部旧资产（版本号变动会留下历史文件），保证始终只有一份最新产物；`target_commitish` 指向当前 commit，使 nightly tag 跟随最新构建。
- **Release 说明改为结构化正文**：用 `generate_release_notes` 自动生成的提交列表可读性差，改为自生成 `notes.md`，含「构建信息」表（版本 / 提交 / 构建号 / 触发方式 / 流水线链接）与「产物清单」表（文件名 + 大小），nightly 版本额外标注滚动预发布提示。
- **打包 release 号恒为 1**：此前非 tag 构建以 `GITHUB_RUN_NUMBER` 作为 release 号，导致每次构建的 ipk/apk 文件名都不同、nightly 资产只增不减。现统一为 1，资产名稳定可覆盖，构建号改由 Release 标题承载。

### 文档

- **README 突出简单易用、零门槛**：导语改为强调「下载即用 + WebUI 图形化操作」的产品定位；新增「三步上手（零门槛）」章节，以表格给出 Windows / Debian / OpenWrt 三平台的下载 → 运行 → 使用最小路径（双击运行、两条命令、装包启用），并说明全程仅需浏览器操作、无需命令行与配置文件；功能特性列表新增「开箱即用，零门槛」条目。内容均按现有实际功能描述，未涉及代码变更。
- **README 移除未实际发布的架构描述**：「多平台 / 多架构」表格删除 `arm_cortex-a7/a9`（armv7 musl）、`mipsel_24kc`、`mips_24kc` 三行及 mips/mipsel 需改用 `native-tls` 的说明——CI 实际仅构建并发布 x86_64 与 aarch64 两个 musl 架构，文档与实际发布物保持一致。

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
