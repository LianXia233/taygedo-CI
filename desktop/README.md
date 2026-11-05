# taygedo 桌面单包（Windows）

原生 Rust **GPUI** 界面 + **同进程内嵌** axum 服务，发布物只有一个 `taygedo.exe`。

## 为什么是单包

早期版本发布两个可执行文件：`taygedo-rs.exe`（回环服务）与 `taygedo-desktop.exe`
（GPUI 客户端），由客户端 `spawn` 服务子进程。这带来一个真实故障：客户端退出时
子进程不会被回收，服务继续常驻，**Windows 于是锁住 `taygedo-rs.exe` 的映像文件**，
用户连解压目录都删不掉（报「文件正在使用中」）。

单包把服务放进界面进程内：

- 不存在子进程，关窗口即进程退出，所有文件句柄随之释放；
- 启动不再需要"拉起服务 → 轮询 `/api/meta` 直到可达"，端口绑定完成即刻可用；
- 端口已被其他实例占用时，新进程**复用**既有实例，而不是抢占或报错退出。

## 架构

```
desktop/src/
    main.rs          # composition root：解析数据目录 → 启动内嵌服务 → 开窗
    client.rs        # 回环 REST 客户端（无 Authorization 头，401/403 明确报错）
    state.rs         # 全部状态与动作：账号/日志/配置/弹窗/Toast/倒计时/轮询
    theme.rs         # 明暗两套调色板，逐项对齐 WebUI 的 --md-* 设计令牌
    views/
        mod.rs          # 根视图：页头 / 统计行 / 主区 / 弹窗层 / Toast 层
        cards.rs        # 账号卡片与空状态
        logs.rs         # 运行日志面板（账号筛选标签 + 暗色终端区）
        add_account.rs  # 添加账号弹窗（密码 / 验证码双模式）
        settings.rs     # 全局设置弹窗
```

内嵌服务来自主仓库的 **lib 目标**（`taygedo`，按 `desktop` feature 编译）：那套
路由没有 WebUI、没有静态资源、没有登录/改密/握手，鉴权边界是「只接受回环来源」。

## 界面

界面结构与交互流程与仓库里的 WebUI（`src/ui.html`）保持一致：

- 页头：主题切换 / 设置 / 添加账号
- 统计：总账号、今日已签到、待签到
- 账号卡片：角色名、签到状态徽章、手机号与 UID、每日签到时间（回车保存）、
  立即签到、删除（带确认弹窗）
- 运行日志：账号筛选标签、级别着色、3 秒轮询自动刷新
- 弹窗：添加账号（密码 / 验证码，验证码带 60 秒倒计时）、全局设置、删除确认
- Toast：底部浮动提示，2.6 秒自动消失

桌面端**不显示**传输加密、修改密码、退出登录三项——它们属于 WebUI / OpenWrt 的
鉴权模型，桌面单包没有登录态。

## 构建

需要 **MSVC 工具链 + Windows SDK**（GPUI 走 DirectX 11）：

```powershell
cd desktop
cargo build --release --locked
# 产物：desktop/target/release/taygedo.exe
```

该 crate 是独立 workspace（自带 `Cargo.lock`），会把主仓库作为 path 依赖编译进来，
因此**一次构建即得单包**，不需要先单独编译服务端。GPUI 依赖不会进入 Linux /
OpenWrt / Docker 的构建图。

Linux / macOS 上构建需要额外系统依赖（X11 + Wayland 开发包 / Xcode），仅用于开发
调试；发布物只面向 Windows。

## 运行

双击 `taygedo.exe`。数据默认写入 **exe 同目录的 `data/`**（绿色版行为，不依赖
当前工作目录，避免从快捷方式启动时把数据写进意外位置）。

| 环境变量 | 默认 | 说明 |
| :--- | :--- | :--- |
| `TAYGEDO_DATA_DIR` | `exe 同目录/data` | 数据目录 |
| `TAYGEDO_API_BASE` | `http://127.0.0.1:8787` | 覆盖界面连接的回环地址 |

监听地址固定为 `127.0.0.1:8787`（编译期常量 `taygedo::DESKTOP_LISTEN`）：桌面单包
的请求全部来自本进程，任何 LAN 可达性都是多余攻击面。

## 安全边界

- 免鉴权：不收集口令、不签发 token。安全性来自「只监听回环 + 只接受回环来源」，
  配合 Windows 账户隔离（主密钥由 DPAPI 保护）。
- **不适用于远程 / LAN 访问**。需要远程管理请使用 Linux / Docker 的 `webui` 形态
  （账号密码 + Bearer token + 应用层加密 v2）。

## 卸载

关闭窗口 → 进程退出 → 直接删除目录。若任务管理器里仍有 `taygedo.exe` 残留，
结束该进程后再删。
