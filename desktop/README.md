# taygedo 桌面客户端（GPUI 原生）

Windows 端替代内置 WebUI 的原生桌面客户端，使用与 FluxDown 同款 GPUI
（`gpui-fast` + `gpui-component`）栈实现。它是**独立 crate**：`Cargo.toml`
中的空 `[workspace]` 让本目录拥有自己的 `Cargo.lock`，不会污染主服务
`taygedo-rs` 的 `--locked` 构建，也不会改动主仓库的 `Cargo.toml` / `Cargo.lock`。

## 与主服务的关系

- 主服务需以 **`desktop` 形态**编译产出（`cargo build --release --no-default-features
  --features desktop`）。该形态在**编译期**强制免鉴权、只监听 `127.0.0.1`、
  仅放行回环来源；不提供首页、静态资源、加密握手，也不提供
  `/api/login`、`/api/password`、`/api/logout`。
- 桌面客户端**免鉴权直连**：打开即连本机回环 API，普通 JSON 请求，
  **不带 `Authorization` 头**，没有登录页、没有口令输入、没有 token、没有改密页。
  若请求返回 401 / 403，客户端会在状态栏明确报错，不会尝试登录。
- 默认基址 `http://127.0.0.1:8787`。启动时先探测 `GET /api/meta`；不可达时按
  `std::env::current_exe()` 同目录查找服务程序并拉起：
  - Windows：`taygedo-rs.exe`
  - Linux / macOS：`taygedo-rs`
  - 拉起时设置 `TAYGEDO_LISTEN=127.0.0.1:8787`、`TAYGEDO_CRYPTO_POLICY=never`；
    Windows 子进程用 `CREATE_NO_WINDOW`（`0x0800_0000`）避免闪出控制台黑窗。
  - 找不到服务程序或拉起后仍不可达，界面显示明确错误与「重新连接」按钮，
    **不会静默失败**。
- 客户端只使用 `std::path` / `std::env` 等跨平台 API，不硬编码系统绝对路径，
  不依赖特定 Shell / 发行版，不写入任何凭据或密钥。

## 安全边界（重要）

Windows 桌面形态**免鉴权**的信任前提是：

1. 服务**只监听 `127.0.0.1`**（回环），不暴露到 LAN / 公网；
2. 鉴权中间件**只接受回环来源**，非回环请求一律 403 `untrusted_source`；
3. 信任依赖操作系统账户隔离，而不是口令或 token。

因此该形态**不适用于远程 / LAN 访问**，也不应放在反向代理后面供外部使用。
需要远程或跨网段管理时，请使用 Linux / Docker 的 **`webui` 形态**（内置 WebUI +
账号口令登录 + 应用层加密信封），通过浏览器访问；**WebUI 仅用于
Linux/Docker/远程兼容**，Windows 桌面端一律走本 GPUI 客户端。

## 环境变量

| 变量 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `TAYGEDO_API_BASE` | `http://127.0.0.1:8787` | 覆盖回环 API 基址（例如端口被占用时）。服务地址也可在「设置」页修改并重新连接。 |

## 构建

```sh
cd desktop
cargo build --release
```

生成的独立可执行文件在 `desktop/target/release/taygedo-desktop`
（Windows 为 `taygedo-desktop.exe`）。把它和 `taygedo-rs(.exe)` 放在同一目录，
双击桌面客户端即可：探测不到服务时会自动拉起同目录的服务进程。

`cargo fetch` / `cargo metadata` 只解析本 crate 的依赖；`cargo check` 也只检查
本 crate，不会影响主服务。

## 平台要求

- **Windows**：MSVC 工具链 + Windows SDK + DirectX 11（GPUI 的 Windows 渲染后端依赖
  `windows` crate 与 DirectWrite / Direct3D）。使用 `#![cfg_attr(target_os = "windows",
  windows_subsystem = "windows")]` 避免控制台黑窗；拉起服务子进程时设置
  `CREATE_NO_WINDOW`。
- **Linux**：需安装 X11 / Wayland 开发库（`libx11`、`libxkbcommon`、`libwayland`、
  `libxkbcommon-x11` 等，取决于所用后端）与 fontconfig 开发库；GPUI 平台 crate 开启了
  `x11` / `wayland` / `font-kit` / `runtime_shaders` 特性。
- **macOS**：需 Xcode 与 Command Line Tools；应用入口使用
  `Application::new_inaccessible(gpui_platform::current_platform(false))` 关闭
  GPUI 无障碍以规避 Touch Bar 相关的窗口释放闪退。

## 分层

```
desktop/
  Cargo.toml        # 独立 workspace、GPUI 依赖栈
  Cargo.lock        # 本 crate 独立锁文件
  README.md
  src/
    main.rs         # composition root：Application / 开窗 / 根视图 / 基址解析、
                    # /api/meta 探测与拉起服务子进程（Windows CREATE_NO_WINDOW）
    client.rs       # 免鉴权回环 REST 客户端（meta / accounts / signin / schedule / logs / config）
    state.rs        # Entity 状态 + 异步命令（独立 tokio runtime，结果回写并 cx.notify）
    views/
      mod.rs        # 根视图外壳、页面切换、状态栏
      connection.rs # 连接状态 / 错误与「重新连接」（不收集任何凭据）
      dashboard.rs  # 主面板：账号列表、刷新、手动签到、新增/删除账号、日志
      settings.rs   # 配置与状态：服务地址、自动刷新间隔、后端配置展示
```

异步命令全部投递到独立 tokio runtime 执行，结果经通道回传后在 GPUI 前台
写回 `Entity` 并 `cx.notify()`，不阻塞 UI 线程。
