# 塔吉多自动签到 (Rust 版)

基于 Rust 重写的塔吉多（幻塔 / 异环等）每日自动签到工具，附带一个现代化、**带登录鉴权**、**手机/PC 自适应**的 WebUI 管理界面，并支持 **OpenWrt 主线**集成（含 LuCI）。

> 上游参考：[zzstar101/taygedo-auto-attendance](https://github.com/zzstar101/taygedo-auto-attendance)（TypeScript 版）。本版将其核心逻辑用 Rust 重写。

## 功能特性

- **多账号**：任意数量的游戏账号，各自独立登录态。
- **两种登录方式**：
  - 密码登录（密码用 `scrypt + AES-256-GCM` 加密后落盘，与上游格式兼容）。
  - 短信验证码登录（WebUI 一键「发送验证码」→ 输入 → 登录）。
- **每日定时签到**：每个账号可单独设置每天的签到时间（`HH:MM`，北京时间），也可设置全局默认时间。
- **完整签到链路**：APP 签到、逐游戏签到（幻塔 1256 / 异环 1289 等，显示中文游戏名）、金币任务（签到/浏览/点赞/分享）、云异环时长。
- **幽灵角色修复**：优先使用战绩卡（`getGameRecordCards`）作为角色↔游戏权威映射，避免 `getGameRoles` 返回幽灵角色导致整账号 `code=5050` 失败。
- **会话自动续期**：`accessToken` 失效时自动 `refreshToken` → 失效再走 `laohuToken` 重建 → 有密码则密码重登。
- **登录鉴权**：WebUI 与所有 API 需要账号密码登录（默认 `admin / admin`，`sha256` 加盐哈希），token 有效期 7 天，支持在线修改账号密码。
- **响应式界面**：手机 / PC 自适应布局，深浅色主题。
- **实时日志**：WebUI 内置带时间戳的详细运行日志。

## 界面

访问 `http://<host>:8787`，默认账号密码 **`admin / admin`**（登录后请在「设置」中修改）：

- 顶部统计：总账号 / 今日已签 / 待签到。
- 账号卡片：显示游戏角色、每日签到时间（可改）、「立即签到」「删除」。
- 「添加账号」弹窗：密码 / 验证码两种登录。
- 「全局设置」：默认签到时间、金币任务、云时长开关、分享平台、修改密码、退出登录。
- 右侧实时运行日志。

## 运行方式

### 方式一：Docker（推荐）

```bash
docker compose up -d --build
```

运行后访问 `http://localhost:8787`，数据持久化在 `./data`。

### 方式二：本机 cargo

```bash
cargo run --release
```

环境变量：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `TAYGEDO_LISTEN` | `0.0.0.0:8787` | 监听地址 |
| `TAYGEDO_DATA_DIR` | `data` | 数据目录 |
| `TAYGEDO_WEB_PASSWORD` | `admin` | WebUI 初始登录密码（账号默认 `admin`） |

### 方式三：Windows 编译

本项目在 Windows 上使用 GNU 工具链编译：

```bash
rustup toolchain install stable-x86_64-pc-windows-gnu
cargo +stable-x86_64-pc-windows-gnu build --release
```

## 多平台 / 多架构

使用 musl 静态链接，支持 OpenWrt 主流路由器架构。已提供 GitHub Actions 工作流（`.github/workflows/build.yml`），打 tag 或手动触发即自动交叉编译并发布：

| OpenWrt 架构 | Rust target | 说明 |
| --- | --- | --- |
| x86_64 | `x86_64-unknown-linux-musl` | 软路由 / 虚拟机 |
| aarch64 (cortex-a53/a72) | `aarch64-unknown-linux-musl` | Rockchip、MT7986 等新平台 |
| arm_cortex-a7/a9 | `armv7-unknown-linux-musleabihf` | IPQ40xx 等 |
| mipsel_24kc | `mipsel-unknown-linux-musl` | MT7621（需 `native-tls`，见下） |
| mips_24kc | `mips-unknown-linux-musl` | MT7620（需 `native-tls`，见下） |

> 说明：默认 TLS 后端为 `rustls`（`aws-lc-rs`），对 x86_64 / aarch64 / arm 支持良好；对 **mips / mipsel**，`aws-lc-rs` 不支持，需将 `Cargo.toml` 中 `reqwest` 改为 `features = ["native-tls"]`（OpenWrt 的 libopenssl）后编译。

## OpenWrt 集成（LuCI）

`openwrt/luci-app-taygedo/` 提供了完整 LuCI 包，安装后可在 **LuCI → 服务 → 塔吉多签到** 里：

- 查看运行状态，一键打开内置 WebUI。
- 配置：启用开关、监听端口、数据目录、Web 登录密码、默认签到时间（UCI `config taygedo`）。
- init.d 脚本（procd）自动拉起/守护 `taygedo-rs`，支持 reload。

结构：

```
openwrt/luci-app-taygedo/
├── Makefile                       # 包定义（默认预编译下载，源码编译见注释）
├── luasrc/
│   ├── controller/taygedo.lua     # 菜单 + 路由
│   ├── model/cbi/taygedo.lua      # UCI 配置表单
│   └── view/taygedo/status.htm    # 状态页
└── root/
    ├── etc/config/taygedo         # UCI 默认配置
    ├── etc/init.d/taygedo         # procd 守护脚本
    └── usr/share/
        ├── luci/menu.d/taygedo.json
        └── rpcd/acl.d/luci-app-taygedo.json
```

使用：把该目录放到 `package/` 或 feeds 中，`make menuconfig` 勾选 `LuCI → Applications → luci-app-taygedo` 后编译。二进制获取方式：

- **默认（预编译下载）**：Makefile 从 GitHub Release 下载对应架构的 musl 二进制（由上面的 CI 产出）。
- **源码编译**：需要 `openwrt/packages` 的 `lang/rust` feed，见 Makefile 末尾注释。

## 数据与兼容性

- `data/accounts.json`：账号列表，字段与上游 `accounts.json` 完全兼容。可直接把上游已登录的账号文件复制过来复用（`refreshToken` / `laohuToken` 会自动续期）。
- `data/config.json`：全局配置（凭据密钥、默认签到时间、各账号签到时间、开关、Web 密码哈希）。
- `data/state.json`：每日签到状态（按「账号 + 日期」去重，避免重复签到）。

## API 一览

除 `/api/login` 外，所有 API 需携带 `Authorization: Bearer <token>`（或 `Cookie: taygedo_token=<token>`）。

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/login` | 登录 `{username, password}` → `{token}` |
| POST | `/api/password` | 修改账号密码 `{username?, old_password, new_password}` |
| GET | `/api/accounts` | 账号列表（敏感字段已脱敏） |
| POST | `/api/accounts` | 登录账号 `{phone, mode, password?, captcha?, name?}` |
| DELETE | `/api/accounts/{id}` | 删除账号 |
| POST | `/api/accounts/{id}/signin` | 手动签到 `{force?}` |
| POST | `/api/accounts/{id}/schedule` | 设置签到时间 `{time:"HH:MM"}` |
| POST | `/api/send-code` | 发送短信验证码 `{phone}` |
| GET/POST | `/api/config` | 读取 / 更新全局配置 |
| GET | `/api/logs?limit=200` | 运行日志 |

## 目录结构

```
src/
├── main.rs        # 入口：启动服务 + 定时调度
├── api.rs         # 塔吉多/老虎 API 客户端（签名、加密、请求）
├── protocol.rs    # MD5 签名、AES-128-ECB、ds 校验、表单编码
├── runner.rs      # 签到核心逻辑
├── crypto.rs      # scrypt + AES-256-GCM 凭据加密、登录密码哈希
├── service.rs     # 应用状态、鉴权会话、业务编排
├── scheduler.rs   # 每日定时调度
├── login.rs       # 设备身份 / 账号 id 生成
├── models.rs      # 数据模型
├── store.rs       # 文件存储
├── web.rs         # HTTP 路由 + 鉴权中间件 + 处理器
└── ui.html        # WebUI（自包含单文件，响应式）
openwrt/luci-app-taygedo/   # OpenWrt / LuCI 集成
.github/workflows/build.yml # 多架构交叉编译 CI
```

## 免责声明

本项目仅供学习与个人自用，请遵守相关平台的服务条款。使用本工具产生的任何后果由使用者自行承担。
