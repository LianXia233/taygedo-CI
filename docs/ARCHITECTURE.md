# 架构契约：平台抽象层与 GPUI 桌面交互层

本文是重构后的**规范性文档**：描述依赖方向、平台边界、构建形态与安全约束。
行为说明以源码为准，本文用于约束后续改动不要重新把平台判断散回业务代码。

## 1. 三种构建形态

形态由 Cargo feature 在**编译期**决定，互斥且必须三选一（`src/main.rs` 用
`compile_error!` 强制）。运行期不做形态探测。

| 形态 | feature | 交互层 | 鉴权边界 | 编译进产物的前端资产 |
| :--- | :--- | :--- | :--- | :--- |
| `webui`（默认） | `webui` | 浏览器 WebUI（Linux / Docker / 远程） | Bearer token + 来源白名单 | 首页、`static/`、v2 加密信封、Ed25519 签名清单 |
| `openwrt` | `openwrt` | LuCI 视图 + rpcd/ucode | LuCI/ubus 会话 + 来源白名单 | 无 |
| `desktop` | `desktop` | 原生 Rust GPUI 客户端（Windows） | **无鉴权**：只监听回环 + 只接受回环来源 | 无 |

`desktop` 形态不注册 `/`、`/static/*`、`/static/manifest.json`、
`/api/crypto/handshake`、`/api/secure/identity`、`/api/login`、
`/api/password`、`/api/logout`。这些代码**没有编译进产物**，不是"存在但 404"。
`/api/meta` 返回 `edition=desktop`、`webui_enabled=false`、
`auth_mode=loopback`、`crypto_required=false`。

WebUI 保留为 Linux / Docker / 远程访问的兼容通道；LuCI 形态不变。

## 2. 平台抽象层

### 2.1 唯一边界

`src/platform/` 是**唯一允许出现 `cfg(windows)` / `cfg(unix)` / 平台 API 的位置**。
业务模块（`service` / `store` / `web` / `crypto_storage` / `main`）通过
`platform::current()` 访问能力，不直接引用 `windows-sys`、`libc`、
`std::os::windows` / `std::os::unix` 或任何系统可执行文件。

测试模块中的 `#[cfg(unix)]` 仅用于裁剪平台专属断言，不属于业务分支。

### 2.2 能力接口

| Trait | 方法（摘要） | 用途 |
| :--- | :--- | :--- |
| `Platform` | `os_name` / `host_kind` / `supports_os_keystore` / `supports_key_rotation` / `key_store` / `secure_file` / `desktop` | 聚合根，编译期静态分发（`&'static dyn Platform`） |
| `KeyStore` | `load_master_key` / `store_new_master_key` / `read_pending_key` / `write_pending_key` / `promote_pending_key` / `permission_warning` | 主密钥保护与轮换 |
| `fs::SecureFile` | `open_new` / `open_existing` / `validate_metadata` / `fsync_dir` + 默认方法 `write_atomic` / `read_secure` / `cleanup_stale_temp` | 平台无关 I/O 骨架 + 平台相关钩子 |
| `DesktopIntegration` | `open_url` / `configure_command` / `host_kind` | 浏览器打开、子进程平台标志、宿主探测 |

`current()` 按 `cfg` 返回 `windows` / `unix` / `unsupported` 三个静态实例之一；
macOS 复用 Unix 实现，仅在打开 URL 时选择 `open`（低成本兼容）。
`unsupported` 对所有密钥操作返回明确的 `PlatformError::Unsupported`，不静默降级。

### 2.3 平台能力对照

| 能力 | Windows (`platform/windows.rs`) | Unix (`platform/unix.rs`) |
| :--- | :--- | :--- |
| 主密钥保护 | DPAPI（`CryptProtectData` / `CryptUnprotectData`），可用 `TAYGEDO_DPAPI_MACHINE` 切机器作用域 | systemd `CREDENTIALS_DIRECTORY` 凭据优先，否则 0600 文件 |
| 密钥文件布局 | `keys/master.key.dpapi` | `keys/master.key`（42 字节 `TGDKEY01` blob）、`keys/master.key.pending` |
| 写入保护 | `create_new` + `sync_all` + `rename`；读取前拒绝重解析点 | `O_NOFOLLOW` / `O_CLOEXEC` + `fstat` 校验 `nlink` / `mode` / `euid` |
| 目录 `fsync` | 不支持，空操作 | `File::open(dir)?.sync_all()` |
| 轮换 | 不支持（明确报错） | 支持 pending 文件流程 |
| 打开 URL | GUI 处理器（`explorer`），不依赖 `cmd.exe` | `xdg-open`（Linux）/ `open`（macOS），可由 `TAYGEDO_BROWSER` 覆盖 |
| 子进程标志 | `CREATE_NO_WINDOW`（不闪控制台黑窗） | 无 |
| 宿主探测 | 固定 `HostKind::Windows` | 可由 `TAYGEDO_HOST_KIND` 显式声明；否则按固件标记与 `cfg!(target_os = "macos")` 判定 |

### 2.4 依赖方向与错误映射

```text
main / web / service / store / crypto_storage
        │  （只依赖 trait，不出现 OS cfg）
        ▼
src/platform  ──►  PlatformError
        │                 │
        ▼                 ▼
windows / unix / unsupported   crypto_storage::StorageError（From 映射）
```

`crypto_storage::platform` 与 `crypto_storage::atomic` 已降为**薄适配层**：
只做 `Zeroizing` 包装、`Protection` re-export 与错误映射；磁盘字节格式由平台层
常量（`KEY_MAGIC` / `KEY_FORMAT_VERSION` / `KEY_BLOB_LEN` / `encode_key_blob`）
统一定义，**保持与重构前完全一致**，已有数据目录无需迁移。

## 3. GPUI 桌面 UI 层

`desktop/` 是**独立 crate、独立 Cargo.lock**，GPUI 依赖不进入主服务的依赖图，
因此主服务的 `cargo build --locked`、Docker 分层缓存与 OpenWrt 交叉编译都不受
影响。它与主服务的关系是**单向的**：`desktop` 依赖主仓库的 lib 目标 `taygedo`，
反向不存在依赖。

### 3.1 单包：界面与服务同进程

Windows 发布物是**一个** `taygedo.exe`，服务端被静态链接进界面进程：

```text
taygedo.exe
  ├─ GPUI 界面（views / state / theme）
  └─ 内嵌 axum 服务（taygedo::start，绑定 127.0.0.1:8787）
```

早期形态是「客户端 + 同目录 `taygedo-rs.exe`」的双进程，由客户端 spawn 服务。
该设计有一个不可接受的副作用：客户端退出时子进程不随之终止，服务继续常驻，
Windows 于是锁住 `taygedo-rs.exe` 的映像文件，用户删除安装目录会失败。单包把
进程数降为 1，关窗即退出，句柄随之释放。

因此主仓库拆出 `src/lib.rs`：独立服务二进制（`taygedo-rs`）与桌面单包共用同一套
启动逻辑，避免两份实现漂移。`cli_main()`（含启动横幅）只给独立二进制用——Windows
GUI 子系统没有控制台，`println!` 写失败会 panic。

### 3.2 分层

依赖栈与 [FluxDown](https://github.com/zerx-lab/FluxDown) 同款：`gpui-fast`
（以 `gpui` 名引入）+ `gpui-fast-platform` + `gpui-kit` 组件。分层：

```text
main（composition root：Runtime、内嵌服务、窗口）
  ├─ client（本机回环 REST，普通 JSON，无 token、无浏览器加密信封）
  ├─ state（Entity 状态 + 全部动作：数据/弹窗/Toast/倒计时/轮询）
  ├─ theme（明暗两套调色板，对齐 WebUI 的 --md-* 令牌）
  └─ views（页头 / 统计 / 账号卡片 / 日志面板 / 三个弹窗 / Toast）
```

- **打开即用**：桌面形态无登录态。不收集口令、不保存 token。
- **启动**：先在当前运行时绑定 `127.0.0.1:8787` 再开窗，返回即端口就绪，不需要
  轮询 `/api/meta`；端口被既有实例占用且该实例可达时，复用而不抢占。
- **数据目录**：默认 `exe 同目录/data`，不依赖当前工作目录（从快捷方式启动时
  CWD 往往不是 exe 目录），可用 `TAYGEDO_DATA_DIR` 覆盖。
- 路径一律用 `std::path` / `std::env` 跨平台 API，不写死系统盘或安装目录。
- Windows 用 `windows_subsystem = "windows"`，避免控制台窗口。

### 3.3 界面与 WebUI 的一致性

界面结构、文案与交互流程以 `src/ui.html` 为蓝本逐条对齐：页头（主题 / 设置 /
添加账号）、三格统计、账号卡片（签到徽章、手机号与 UID、每日签到时间、立即签到、
删除）、运行日志面板（账号筛选标签、级别着色、3 秒轮询）、添加账号弹窗（密码 /
验证码双模式 + 60 秒倒计时）、全局设置弹窗、删除确认、Toast、明暗主题。

不迁移的三项：传输加密、修改密码、退出登录——它们服务于 WebUI / OpenWrt 的鉴权
模型，桌面单包没有登录态，放上去只会是无效入口。

## 4. 安全边界（重要）

`desktop` 形态的免鉴权**不是**"接口敞开"，而是把信任边界前移到操作系统：

1. 服务默认只监听 `127.0.0.1:8787`（独立二进制的 `TAYGEDO_LISTEN` 可覆盖；桌面
   单包把该地址写成编译期常量 `taygedo::DESKTOP_LISTEN`，不接受运行期改写）；
2. 鉴权中间件要求 `ip.is_loopback()` 为真，且复用统一来源归一化判定；
   不放宽到 `lan_cidrs`，即使误绑 `0.0.0.0`，非回环来源仍返回 403；
3. 主密钥由 DPAPI 绑定当前 Windows 账户，其它账户/机器无法解封；
4. 该形态**不提供远程访问**。远程管理必须用 `webui` 形态（TLS 前置代理 +
   登录 + v2 加密信封）。

## 5. 构建与验证

```sh
# 主服务（三种形态都要过）
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --no-default-features --features openwrt -- -D warnings
cargo clippy --locked --all-targets --no-default-features --features desktop -- -D warnings
cargo test --locked
cargo test --locked --no-default-features --features openwrt
cargo test --locked --no-default-features --features desktop

# 桌面单包（独立 lock；会连带编译主仓库的 lib 目标）
cd desktop && cargo build --release --locked
```

## 6. 工程约定

- 所有源文件、脚本、配置一律 **LF** 行尾；`.gitattributes` 的 `* text=auto eol=lf`
  统一生效（仅 `.ps1` / `.bat` 保留 CRLF）。CRLF 会导致 BusyBox ash / procd 启动失败。
- 核心代码兼容 Linux + Windows；macOS 在低成本前提下兼容（复用 Unix 实现）。
- 不硬编码系统绝对路径；系统相关功能先做平台检测再分支；不依赖特定 Shell
  或发行版工具。宿主类型可由 `TAYGEDO_HOST_KIND` 显式覆盖。
- 主服务与 `desktop/` 的 Cargo.lock 相互独立，任何一侧新增依赖不触发另一侧
  `--locked` 失败。
