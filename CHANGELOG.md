# 更新日志 (Changelog)

本文件记录塔吉多自动签到（Rust 版）的所有版本变更。格式参考 [Keep a Changelog](https://keepachangelog.com/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

> 各版本的完整代码差异可对比 Git Tag：`v0.1.0` … `v0.5.0`。

> **关于初始口令的重要提示**：`admin / admin` 这类固定初始口令**仅存在于 0.5.0 之前的历史版本**，
> 下方旧版本条目（如 0.2.0）中的相关描述是当时的事实记录，**不代表当前行为**。
> 0.5.0 起首次启动一律由 CSPRNG 随机生成初始口令，仅在启动横幅打印一次。
> 以 [README](README.md) 的说明为准。

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
