# 塔吉多自动签到 (Rust 版)

基于 Rust 重写的塔吉多（幻塔 / 异环等）每日自动签到工具，主打**简单易用、零门槛**：下载一个文件、双击运行，剩下的全部在一个现代化、**带登录鉴权**、**手机/PC 自适应**的 WebUI 图形界面里点点鼠标完成 —— 不需要命令行参数、不需要编辑配置文件、不懂技术也能上手，并支持 **OpenWrt 主线**集成（含 LuCI）。

> 📦 各平台安装包见 [Releases](https://github.com/LianXia233/taygedo-CI/releases)
>
> Releases 分两条轨道：**正式版**（tag 形如 `v0.4.11`，稳定、推荐生产使用）与 **`nightly`**（滚动预发布，每次手动构建覆盖更新，仅供尝鲜）。日常使用请选正式版，或直接访问 [Latest Release](https://github.com/LianXia233/taygedo-CI/releases/latest)。

> 上游参考：[zzstar101/taygedo-auto-attendance](https://github.com/zzstar101/taygedo-auto-attendance)（TypeScript 版，**MIT 许可**）。本版将其核心逻辑用 Rust 重写，上游的 MIT 版权与许可声明保留在 [`src/upstream/`](./src/upstream/)；本项目自身代码以 **GPL-2.0** 发布，详见文末[许可证](#许可证)。

---

## 三步上手（零门槛）

全程只需要浏览器操作：添加账号、设置签到时间、看日志、改配置，都在 WebUI 界面里完成。

| 步骤 | Windows | Debian / Ubuntu | OpenWrt |
| --- | --- | --- | --- |
| ① 下载 | 到 [Releases](https://github.com/LianXia233/taygedo-CI/releases) 下载 zip 并解压 | 下载 `.deb` | 下载对应架构的 `.ipk` / `.apk` |
| ② 运行 | **双击 `taygedo-rs.exe`**（自动打开浏览器） | `sudo dpkg -i taygedo-rs_*.deb && sudo systemctl enable --now taygedo-rs` | 安装后到 LuCI → 服务 → 塔吉多签到 启用 |
| ③ 使用 | 浏览器打开 `http://127.0.0.1:8787`，默认账号密码 `admin / admin` 登录，点「添加账号」即可 | 同左（把地址换成服务器 IP） | LuCI 页面直接管理 |

就这么多 —— 之后每天会在你设定的时间自动签到，无需任何人工干预；手机浏览器同样可以打开管理界面。

---

## 功能特性

- **开箱即用，零门槛**：单文件程序，解压即用；所有功能都有图形界面，无需配置文件、无需命令行知识。
- **多账号**：任意数量的游戏账号，各自独立登录态。
- **两种登录方式**：
  - 密码登录（密码用 `scrypt + AES-256-GCM` 加密后落盘，与上游格式兼容）。
  - 短信验证码登录（WebUI 一键「发送验证码」→ 输入 → 登录）。
- **每日定时签到**：每个账号可单独设置每天的签到时间（`HH:MM`），也可设置全局默认时间。调度器固定使用**北京时间（UTC+8）**，与设备系统时区无关 —— Windows / Linux / OpenWrt 各平台行为完全一致；若进程在设定时间之后才启动（开机、重启），会自动**补签**，不会错过当天任务；每账号每天只触发一次，已签到的账号自动跳过，不会重复签到。
- **完整签到链路**：APP 签到、逐游戏签到（幻塔 1256 / 异环 1289 等，显示中文游戏名）、金币任务（签到/浏览/点赞/分享）、云异环时长。
- **幽灵角色修复**：优先使用战绩卡（`getGameRecordCards`）作为角色↔游戏权威映射，避免 `getGameRoles` 返回幽灵角色导致整账号 `code=5050` 失败。
- **会话自动续期**：`accessToken` 失效时自动 `refreshToken` → 失效再走 `laohuToken` 重建 → 有密码则密码重登。
- **登录鉴权**：WebUI 与所有 API 需要账号密码登录（默认 `admin / admin`，`sha256` 加盐哈希），token 有效期 7 天，支持在线修改账号密码。
- **免鉴权模式（可选）**：设置环境变量 `TAYGEDO_NO_AUTH=1`（OpenWrt 下在 UCI 配置 `option no_auth '1'`）后，WebUI 与 LuCI 页面**无需登录**即可直接使用，适合内网自用场景。
- **响应式界面**：手机 / PC 自适应布局，深浅色主题，背景壁纸（毛玻璃卡片）。
- **实时日志**：WebUI 内置带时间戳的详细运行日志。

## 界面

访问 `http://<host>:8787`，默认账号密码 **`admin / admin`**（登录后请在「设置」中修改）：

- 顶部统计：总账号 / 今日已签 / 待签到。
- 账号卡片：自定义头像、**平台昵称**（无昵称时回退备注名）、每日签到时间（可改）、「立即签到」「删除」。
- 「添加账号」弹窗：密码 / 验证码两种登录。
- 「全局设置」：默认签到时间、金币任务、云时长开关、分享平台、修改密码、退出登录。
- 右侧实时运行日志。

---

## 安装教程

### 下载对应平台的安装包

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows (64 位) | `taygedo-rs-windows-x86_64.zip` | 单文件可执行程序 |
| Debian / Ubuntu (amd64) | `taygedo-rs_<版本>_amd64.deb` | 含 systemd 服务 |
| OpenWrt (x86_64 软路由) | `luci-app-taygedo_<版本>-1_x86_64.ipk` 或 `.apk` | 含 LuCI + 二进制 |
| OpenWrt (ARM64 路由器) | `luci-app-taygedo_<版本>-1_aarch64_cortex-a53.ipk` 或 `.apk` | 含 LuCI + 二进制 |
| Linux 静态 (musl) | `taygedo-rs-<arch>-unknown-linux-musl.tar.gz` | 其他 Linux/容器通用 |

> OpenWrt 24.10 及以上用 **apk** 包，23.05 及以下用 **ipk** 包（opkg）。

### Windows 安装

**1. 下载解压**

在 [Releases](https://github.com/LianXia233/taygedo-CI/releases) 下载 `taygedo-rs-windows-x86_64.zip`，解压得到 `taygedo-rs.exe`。

**2. 运行**

方式一（最简单）：双击 `taygedo-rs.exe`，弹出一个控制台窗口并启动服务。

方式二（推荐，便于自定义）：

```powershell
# 进入解压目录后运行
.\taygedo-rs.exe
```

可选环境变量：

```powershell
$env:TAYGEDO_LISTEN = "0.0.0.0:8787"        # 监听端口（默认 8787）
$env:TAYGEDO_DATA_DIR = "D:\taygedo-data"    # 数据目录（默认 .\data）
$env:TAYGEDO_WEB_PASSWORD = "你的密码"         # 初始登录密码（默认 admin）
.\taygedo-rs.exe
```

**3. 访问 WebUI**

浏览器打开 `http://127.0.0.1:8787`，用 `admin / admin` 登录。

**4. 开机自启（可选）**

- 任务计划程序：创建任务 → 触发器选「登录时」→ 操作填 `taygedo-rs.exe` 完整路径 → 勾选「使用最高权限运行」。
- 启动文件夹：把快捷方式放入 `Win + R` → `shell:startup` 目录。

**5. 防火墙放行（局域网访问用）**

```powershell
netsh advfirewall firewall add rule name="taygedo" dir=in action=allow protocol=TCP localport=8787
```

### Debian 安装

**1. 下载安装**

```bash
# 到 Releases 页面下载最新 deb（文件名带版本号，如 taygedo-rs_0.4.11_amd64.deb）
# https://github.com/LianXia233/taygedo-CI/releases/latest
sudo dpkg -i taygedo-rs_<版本>_amd64.deb
sudo apt-get install -f    # 若有依赖缺失（本项目基本无依赖，通常不需要）
```

**2. 启动并设置开机自启**

```bash
sudo systemctl enable --now taygedo-rs
sudo systemctl status taygedo-rs
```

**3. 自定义配置**

```bash
sudo systemctl edit taygedo-rs
```

在弹出内容中覆盖环境变量：

```ini
[Service]
Environment=TAYGEDO_LISTEN=0.0.0.0:8787
Environment=TAYGEDO_DATA_DIR=/var/lib/taygedo
Environment=TAYGEDO_WEB_PASSWORD=你的初始密码
```

保存后：

```bash
sudo systemctl daemon-reload
sudo systemctl restart taygedo-rs
```

**4. 访问与日志**

浏览器打开 `http://<服务器IP>:8787`，用 `admin / admin` 登录。

```bash
sudo journalctl -u taygedo-rs -f
```

数据默认存储在 `/var/lib/taygedo`（`accounts.json` / `config.json` / `state.json`）。

### OpenWrt 安装

**1. 确定包格式与架构**

| OpenWrt 版本 | 包管理器 | 文件 |
| --- | --- | --- |
| 24.10 及以上 | `apk` | `*.apk` |
| 23.05 及以下 | `opkg` | `*.ipk` |

| 设备/平台 | 架构 | 文件名 |
| --- | --- | --- |
| x86_64 软路由 / 虚拟机 | x86_64 | `..._x86_64.ipk/apk` |
| ARM64 路由（MT7986/Rockchip 等） | aarch64_cortex-a53 | `..._aarch64_cortex-a53.ipk/apk` |

**2. 安装**

```sh
# opkg（23.05 及以下），文件名带版本号，如 luci-app-taygedo_0.4.8-1_x86_64.ipk
opkg install /tmp/luci-app-taygedo_<版本>-1_x86_64.ipk

# apk（24.10 及以上），如 luci-app-taygedo_0.4.9-r1_x86_64.apk
apk add /tmp/luci-app-taygedo_<版本>-r1_x86_64.apk
```

**3. LuCI 页面**

浏览器打开路由器 LuCI → **服务 → 塔吉多签到**，单页即完整管理界面（账号卡片 / 统计 / 运行日志 / 添加账号 / 全局设置），与 WebUI 功能、视觉保持同步。

- **免鉴权模式**（UCI 默认 `option no_auth '1'`，**OpenWrt 专享**）：LuCI 页面免登录直连后端 API，直接管理账号。
- **未开免鉴权**：页面显示引导页，点右上「**外部 WebUI**」按钮跳转 `:8787` 独立管理界面（或在路由器执行 `uci set taygedo.main.no_auth=1 && uci commit taygedo && /etc/init.d/taygedo restart` 开启免鉴权）。
- **职责划分（LuCI 与 WebUI 解耦）**：UCI（`/etc/config/taygedo`）只管理**服务级**配置（启用 / 监听端口 / 数据目录 / Web 登录密码 / 免鉴权开关）；**业务级**配置（默认签到时间 / 金币任务 / 云时长 / 分享平台）统一由 `config.json` 管理，LuCI 页面与独立 WebUI 均通过「全局设置」读写同一份数据，二者**数据互通、相互独立、互不影响**——重启服务不会用 UCI 旧值覆盖 WebUI 的修改。

**4. 打开 WebUI**

LuCI 页面右上「**外部 WebUI**」按钮，或浏览器直接访问 `http://<路由器IP>:8787`。免鉴权模式直接进入；未开启时用 `admin / admin` 登录。

**5. 命令行管理（可选）**

```sh
# 服务级配置（UCI）
uci set taygedo.main.enabled=1
uci set taygedo.main.port=8787
uci commit taygedo

# 业务级配置（默认签到时间 / 金币任务 / 云时长 / 分享平台）
# 在 LuCI 页面或 WebUI 的「全局设置」中修改，统一写入 config.json，无需操作 UCI

/etc/init.d/taygedo start|stop|restart|status
logread | grep taygedo
```

---

## 通用使用指南

**登录**：默认 `admin / admin`，首次登录后请在「设置」中修改。

**添加账号**：
- 密码登录：输入手机号 + 密码。
- 验证码登录：输入手机号 → 点「发送验证码」→ 输入短信验证码。

**每日签到时间**：全局默认在「设置」；单账号在账号卡片上直接修改。

**手动签到**：账号卡片点「立即签到」。

**多账号**：重复「添加账号」，每个账号独立登录态与签到时间。

---

## 从源码构建

### Docker（推荐）

```bash
docker compose up -d --build
```

运行后访问 `http://localhost:8787`，数据持久化在 `./data`。

### 本机 cargo

需要 Rust 工具链（stable 即可）。

```bash
cargo run --release
```

环境变量：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `TAYGEDO_LISTEN` | `0.0.0.0:8787` | 监听地址 |
| `TAYGEDO_DATA_DIR` | `data` | 数据目录 |
| `TAYGEDO_WEB_PASSWORD` | `admin` | WebUI 初始登录密码（账号默认 `admin`） |
| `TAYGEDO_DEFAULT_SCHEDULE` | 无 | 覆盖默认签到时间 |
| `TAYGEDO_COIN_TASKS` | 无 | 覆盖金币任务开关（true/false） |
| `TAYGEDO_CLOUD_DURATION` | 无 | 覆盖云时长开关（true/false） |
| `TAYGEDO_SHARE_PLATFORM` | 无 | 覆盖分享平台 |
| `TAYGEDO_NO_AUTH` | `false` | 免鉴权模式（`1/true/yes/on` 开启），WebUI 与 API 无需登录 |

### Windows 编译

```bash
rustup toolchain install stable-x86_64-pc-windows-gnu
cargo +stable-x86_64-pc-windows-gnu build --release
```

---

## 多平台 / 多架构

使用 musl 静态链接。`.github/workflows/build.yml` 打 tag 或手动触发即自动交叉编译并发布以下架构：

| OpenWrt 架构 | Rust target | 说明 |
| --- | --- | --- |
| x86_64 | `x86_64-unknown-linux-musl` | 软路由 / 虚拟机 |
| aarch64 (cortex-a53) | `aarch64-unknown-linux-musl` | Rockchip、MT7986 等新平台 |

---

## OpenWrt / LuCI 集成（开发者）

`openwrt/luci-app-taygedo/` 提供完整 LuCI 包，安装后可在 **LuCI → 服务 → 塔吉多签到** 里：

- **功能与 WebUI 一致**：账号管理、密码/短信验证码登录、每日签到时间、立即签到、运行日志、全局设置（免鉴权模式下隐藏改密）。
- **与 WebUI 解耦**：LuCI 页面不维护后端登录态/token，直连后端 REST API（依赖免鉴权模式，UCI 默认 `no_auth '1'`）；未开免鉴权时显示引导页并提供「外部 WebUI」跳转。
- **服务级配置**（UCI `config taygedo`）：启用开关、监听端口、数据目录、Web 登录密码、免鉴权开关。
- **业务级配置**（默认签到时间、金币任务、云时长、分享平台）：统一由 `config.json` 管理，LuCI 页面与独立 WebUI 通过 `/api/config` 读写同一份数据，数据互通、互不覆盖。
- init.d 脚本（procd）自动拉起/守护 `taygedo-rs`，支持 reload。

```
openwrt/luci-app-taygedo/
├── Makefile                       # 包定义（默认预编译下载，源码编译见注释）
├── htdocs/
│   └── luci-static/resources/view/taygedo/
│       └── status.js              # 现代 LuCI JS 前端（功能对齐 WebUI）
└── root/
    ├── etc/config/taygedo         # UCI 默认配置
    ├── etc/init.d/taygedo         # procd 守护脚本
    └── usr/share/
        ├── luci/menu.d/taygedo.json
        └── rpcd/acl.d/luci-app-taygedo.json
```

使用：把该目录放到 `package/` 或 feeds 中，`make menuconfig` 勾选 `LuCI → Applications → luci-app-taygedo` 后编译。

---

## 数据与兼容性

- `data/accounts.json`：账号列表，字段与上游 `accounts.json` 完全兼容。可直接把上游已登录的账号文件复制过来复用（`refreshToken` / `laohuToken` 会自动续期）。
- `data/config.json`：全局配置（凭据密钥、默认签到时间、各账号签到时间、开关、Web 账号密码哈希）。
- `data/state.json`：每日签到状态（按「账号 + 日期」去重，避免重复签到）。

---

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

---

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
scripts/package.sh          # deb / ipk / apk 打包脚本
```

---

## 更新日志 (Changelog)

完整的版本变更记录已迁移至独立文件 [CHANGELOG.md](./CHANGELOG.md)，本文件不再内嵌更新日志。

## 常见问题

**Q：访问 WebUI 提示无法连接？**
检查服务是否运行、端口是否被防火墙/安全组拦截、监听地址是否为 `0.0.0.0`。

**Q：忘记 WebUI 密码？**
删除数据目录下的 `config.json`（或其中的 `web_username`/`web_password_hash`/`web_password_salt` 字段）后重启，恢复默认 `admin/admin`。

**Q：签到失败 / 登录态失效？**
在 WebUI 中删除该账号并重新登录即可。

**Q：OpenWrt 上如何更新？**
下载新版本对应架构的包，用 `opkg install` / `apk add` 覆盖安装。

**Q：手机能访问吗？**
能，WebUI 已适配手机浏览器。

---

## 免责声明

本项目仅供学习与个人自用，请遵守相关平台的服务条款。使用本工具产生的任何后果由使用者自行承担。


---

## 许可证

| 范围 | 许可证 | 位置 |
| :--- | :--- | :--- |
| 本项目 Rust 实现、WebUI 与 OpenWrt / LuCI 集成 | **GPL-2.0** | 根目录 [`LICENSE`](./LICENSE) |
| 上游 TypeScript 实现（逻辑参考来源） | **MIT** | [`src/upstream/LICENSE-MIT`](./src/upstream/LICENSE-MIT)，Copyright (c) 2026 zzstar101 |

- 本项目为上游 [zzstar101/taygedo-auto-attendance](https://github.com/zzstar101/taygedo-auto-attendance)（MIT）的 Rust 重写版本，新增与改写的代码整体以 **GPL-2.0** 授权。
- 上游是宽松的 MIT 许可，允许其代码被并入 GPL 项目，因此两层许可可以并存：上游归属与 MIT 声明完整保留，不因本项目改用 GPL 而改变。
- 若你只使用上游的原始 TypeScript 版本，请以该项目的 MIT 许可为准；使用本项目（Rust 版）及其衍生代码时，需遵循 GPL-2.0。
