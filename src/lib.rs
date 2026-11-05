//! taygedo 库入口：让「独立服务二进制」与「桌面单包」共用同一份启动逻辑。
//!
//! 存在的唯一理由：Windows 桌面端不再以「GPUI 客户端 + 同目录服务 exe」的双进程
//! 形态发布。双进程的问题是子进程会在客户端退出后继续常驻，Windows 因此锁住
//! `taygedo-rs.exe` 映像，用户连卸载/删除目录都做不到。单包把 axum 服务放进
//! GPUI 进程内，进程退出即全部释放。
//!
//! 构建形态（webui / openwrt / desktop）仍在**编译期**由 feature 决定，库与 bin
//! 共享同一套 feature，不存在运行期探测。
#[cfg(any(
    all(feature = "webui", feature = "openwrt"),
    all(feature = "webui", feature = "desktop"),
    all(feature = "openwrt", feature = "desktop")
))]
compile_error!("构建形态互斥：一次只能选择 webui / openwrt / desktop 之一");

#[cfg(not(any(feature = "webui", feature = "openwrt", feature = "desktop")))]
compile_error!("必须显式选择构建形态：webui / openwrt / desktop");

mod api;
mod constants;
mod crypto;
mod crypto_storage;
mod login;
#[cfg(feature = "openwrt")]
mod luci_session;
mod models;
mod platform;
mod protocol;
mod runner;
mod scheduler;
mod service;
mod store;
mod time;
mod web;
mod web_security;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use tokio::net::TcpListener;

pub use service::AppState;

/// 桌面单包使用的回环监听地址。
///
/// 与 WebUI 形态的 `0.0.0.0:8787` 不同：桌面端的请求全部来自同机 GPUI 进程，
/// 监听回环即可，任何 LAN 可达性都是多余攻击面。
pub const DESKTOP_LISTEN: &str = "127.0.0.1:8787";

/// 已启动服务的句柄。
///
/// `shutdown()` 触发 axum 的优雅关闭；句柄被 drop 不会终止服务（避免调用方
/// 无意中把服务停掉），必须显式调用。
pub struct ServiceHandle {
    /// 实际监听地址（端口传 0 时由系统分配，这里返回真实端口）。
    pub listen: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ServiceHandle {
    /// 优雅关闭：停止接受新连接并等待现有请求结束。
    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// 绑定端口并在**当前 tokio 运行时**上启动 HTTP 服务。
///
/// 返回时监听已就绪（端口已绑定），调用方可以立刻向该地址发请求，不需要轮询。
/// 因此桌面单包不必再"拉起服务 → 等 /api/meta 可达"。
///
/// 失败原因全部是字符串：这是给 UI 直接展示的文案，不需要类型化。
pub async fn start(listen: SocketAddr, data_dir: PathBuf) -> Result<ServiceHandle, String> {
    let state = AppState::new(data_dir)?;
    state.push_log(
        "info",
        format!(
            "塔吉多自动签到已启动（进程内服务），数据目录：{}",
            state.store.data_dir().display()
        ),
    );
    let (listener, app, addr) = bind(state, listen).await?;
    Ok(spawn_serve(listener, app, addr))
}

/// 绑定监听端口并构造路由，但不启动服务循环。
async fn bind(
    state: std::sync::Arc<AppState>,
    listen: SocketAddr,
) -> Result<(TcpListener, Router, SocketAddr), String> {
    scheduler::spawn(state.clone());
    let app = web::router(state);
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|e| format!("绑定 {listen} 失败：{e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("读取监听地址失败：{e}"))?;
    Ok((listener, app, addr))
}

fn spawn_serve(listener: TcpListener, app: Router, addr: SocketAddr) -> ServiceHandle {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let served = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = rx.await;
        })
        .await;
        if let Err(e) = served {
            eprintln!("[error] 服务运行失败：{e}");
        }
    });
    ServiceHandle {
        listen: addr,
        shutdown: Some(tx),
    }
}

/// 独立服务二进制（`taygedo-rs`）的入口。
///
/// 返回进程退出码。桌面单包**不**调用它：那条路径会打印启动横幅，而 Windows
/// GUI 子系统的进程没有控制台，`println!` 写失败会 panic。
pub fn cli_main() -> i32 {
    let data_dir = std::env::var("TAYGEDO_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    // 桌面形态默认只监听回环：安全边界是"同机 + 回环来源"，不暴露到 LAN。
    // 其余形态保持兼容默认值（可由 TAYGEDO_LISTEN 显式覆盖）。
    let listen = std::env::var("TAYGEDO_LISTEN").unwrap_or_else(|_| {
        if cfg!(feature = "desktop") {
            DESKTOP_LISTEN.to_string()
        } else {
            "0.0.0.0:8787".to_string()
        }
    });

    // 运维子命令：密钥轮换。必须在建立 AppState 之前单独处理，
    // 因为它要求"先取锁 → 重新加密全部文件 → 提升新密钥"的独占流程。
    if std::env::args().any(|a| a == "--rotate-storage-key") {
        return rotate_storage_key_cli(&data_dir);
    }

    let bind_addr: SocketAddr = match listen.parse() {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("[fatal] TAYGEDO_LISTEN 格式应为 host:port：{e}");
            return 1;
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("[fatal] 无法创建异步运行时：{e}");
            return 1;
        }
    };

    runtime.block_on(async move {
        // 存储层初始化失败 = 不可启动。绝不带着"没有密钥/没迁移完"的状态
        // 继续对外提供服务——那会在下一次写入时静默覆盖掉用户数据。
        let state = match AppState::new(PathBuf::from(&data_dir)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[fatal] 启动失败：{e}");
                eprintln!("[fatal] 数据目录：{data_dir}");
                eprintln!(
                    "[fatal] 请勿删除数据目录中的密文文件；恢复步骤见 README「加密存储 / 故障恢复」。"
                );
                return 1;
            }
        };

        state.push_log(
            "info",
            format!(
                "塔吉多自动签到 Rust 版已启动，数据目录：{}",
                state.store.data_dir().display()
            ),
        );

        let (listener, app, addr) = match bind(state.clone(), bind_addr).await {
            Ok(parts) => parts,
            Err(e) => {
                eprintln!("[fatal] {e}");
                return 1;
            }
        };

        print_banner(&state, &data_dir, addr).await;

        let served = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;

        match served {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("[fatal] 服务运行失败：{e}");
                1
            }
        }
    })
}

/// 启动横幅。
///
/// 形态在**编译期**已确定，这里只按 feature 选择文案，不含任何运行期探测。
async fn print_banner(state: &AppState, data_dir: &str, bind_addr: SocketAddr) {
    #[cfg(feature = "webui")]
    let display_url = if bind_addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{}", bind_addr.port())
    } else {
        format!("http://{}", bind_addr)
    };
    #[cfg(feature = "openwrt")]
    let display_url = "LuCI 菜单：服务 → 塔吉多签到".to_string();
    #[cfg(feature = "desktop")]
    let display_url = "GPUI 桌面客户端（本机回环 API）".to_string();

    let listen_note = if bind_addr.ip().is_unspecified() {
        format!("监听所有接口 0.0.0.0:{}", bind_addr.port())
    } else {
        format!("监听 {bind_addr}")
    };

    #[cfg(feature = "webui")]
    let auth_note = "账号密码登录 (Bearer token)";
    #[cfg(feature = "openwrt")]
    let auth_note = if state.no_auth {
        "免鉴权模式 (内网放行)"
    } else if service::luci_session_required() {
        "LuCI 会话 (OpenWrt 鉴权)"
    } else {
        // 排障降级（TAYGEDO_LUCI_SESSION_CHECK=0）：OpenWrt 上没有自有口令，
        // 不能显示"账号密码登录"，否则会把排障者引到不存在的入口。
        "仅受信来源 (LuCI 校验已关闭)"
    };
    #[cfg(feature = "desktop")]
    let auth_note = "免鉴权（仅本机回环来源）";

    #[cfg(feature = "webui")]
    let crypto_note = match state.config.read().await.crypto_policy.as_str() {
        "always" => "应用层加密: 强制 (v2)",
        "never" => "应用层加密: 已关闭 (明文传输)",
        _ => "应用层加密: 内网明文/外网强制",
    };
    // OpenWrt / 桌面形态都不存在浏览器侧消费者：不挂加密信封中间件，
    // 也没有握手端点。
    #[cfg(feature = "openwrt")]
    let crypto_note = "应用层加密: 不适用 (内部通道)";
    #[cfg(feature = "desktop")]
    let crypto_note = "应用层加密: 不适用 (本机回环)";

    // 横幅按**终端显示宽度**对齐：全角字符占 2 列，`{:<40}` 这类按 char
    // 计数的格式会把中文行整体挤歪（历史上这个框一直是斜的）。
    const BOX_INNER: usize = 50;
    println!("╔{}╗", "═".repeat(BOX_INNER));
    print_box_row("塔吉多自动签到 (Rust) 已启动", BOX_INNER);
    print_box_row("", BOX_INNER);
    print_box_row(&format!("访问地址: {display_url}"), BOX_INNER);
    print_box_row(&listen_note, BOX_INNER);
    print_box_row(&format!("鉴权: {auth_note}"), BOX_INNER);
    print_box_row(crypto_note, BOX_INNER);
    print_box_row(&format!("数据目录: {data_dir}"), BOX_INNER);
    print_box_row(
        &format!(
            "配置加密: XChaCha20-Poly1305 / {}",
            state.store.key_protection().as_str()
        ),
        BOX_INNER,
    );
    print_box_row(
        &format!("Key ID  : {}", state.store.key_id_hex()),
        BOX_INNER,
    );
    // 传输层身份指纹：供**带外核对**（TOFU 首次信任窗口的唯一缓解手段）。
    // 打印它是安全的：指纹是公钥的哈希，不是密钥材料。
    //
    // 仅 webui 形态存在握手端点；其它形态没有 TOFU 场景，打印一个无消费者的
    // 指纹只会让人误以为需要核对。
    #[cfg(feature = "webui")]
    print_box_row(
        &format!("身份指纹: {}", state.crypto.identity_fingerprint()),
        BOX_INNER,
    );
    println!("╚{}╝", "═".repeat(BOX_INNER));

    // OpenWrt 形态：说明入口与鉴权归属，避免使用者继续找 WebUI 端口。
    #[cfg(feature = "openwrt")]
    {
        println!();
        println!("[OpenWrt 集成形态] 本二进制不含 WebUI、不含浏览器侧加密前端。");
        println!("  唯一入口：LuCI 菜单「服务 → 塔吉多签到」");
        println!("  数据通道：rpcd/ucode 在服务端调用本机回环接口，浏览器不直连本端口");
        if service::luci_session_required() {
            println!("  鉴权：由 LuCI/ubus 会话负责（本服务只接受回环或受信内网来源）");
        } else {
            println!("  鉴权：LuCI 会话校验已关闭（排障降级），仅按来源放行");
        }
    }

    // 桌面形态：Windows 端已放弃 WebUI，交互层由单包内的 GPUI 界面承担。
    #[cfg(feature = "desktop")]
    {
        println!();
        println!("[桌面形态] 本二进制不含 WebUI、不含任何浏览器侧资产。");
        println!("  正式分发形态：单包 taygedo.exe（GPUI 界面内嵌本服务，同进程）；");
        println!("  本二进制是该服务端的独立形式，用于排障或自带界面之外的自启动场景。");
        println!("  安全边界：仅本机回环监听 + 回环来源校验，不提供远程/LAN 访问");
    }

    // 初始随机口令：仅在 stdout 出现一次，**不写入日志缓冲区**（日志可经 API 读取）。
    #[cfg(feature = "webui")]
    if let Some(pwd) = state.initial_password.as_deref() {
        println!();
        println!("┌────────────────────────────────────────────────────────────┐");
        println!("│  首次启动：已生成随机 WebUI 登录口令（请立即记录并妥善保管）  │");
        println!("│                                                            │");
        println!("│    账号: admin                                             │");
        println!("│    口令: {:<48} │", pwd);
        println!("│                                                            │");
        println!("│  该口令仅在此处显示一次，不会写入日志或配置文件明文。        │");
        println!("│  登录后请立即在「设置 → 修改账号密码」中修改。               │");
        println!("└────────────────────────────────────────────────────────────┘");
        println!();
    }

    // 可选的浏览器自启动：由**平台抽象层**完成，业务侧不出现 cfg(windows)，
    // 也不依赖 cmd.exe / 任何 Shell。默认关闭，桌面形态不需要浏览器。
    #[cfg(feature = "webui")]
    if env_flag("TAYGEDO_OPEN_BROWSER") {
        match platform::current().desktop().open_url(&display_url) {
            Ok(()) => state.push_log("info", format!("已打开 WebUI：{display_url}")),
            Err(e) => state.push_log("warn", format!("打开 WebUI 失败：{e}")),
        }
    }
}

#[cfg(feature = "webui")]
fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// `--rotate-storage-key` 实现：轮换配置加密主密钥。
///
/// 流程（详见 `crypto_storage::migration::rotate_key`）：
/// 新密钥先落 `keys/master.key.pending` → 逐文件用旧密钥解密、新密钥重加密
/// → 全部成功后把 pending 提升为正式主密钥。中途崩溃下次启动会自动续做。
fn rotate_storage_key_cli(data_dir: &str) -> i32 {
    let path = PathBuf::from(data_dir);
    let mut store = match store::Store::new(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[fatal] 无法打开数据目录：{e}");
            return 1;
        }
    };
    let old = store.key_id_hex();
    match store.rotate_storage_key() {
        Ok(new_id) => {
            println!("主密钥已轮换：{old} -> {new_id}");
            println!("旧的加密备份（*.rotate-bak-{old}）保留在数据目录中，确认无误后可手动删除。");
            0
        }
        Err(e) => {
            eprintln!("[fatal] 密钥轮换失败：{e}");
            eprintln!("[fatal] 数据未被破坏；若存在 keys/master.key.pending，下次启动会自动续做。");
            1
        }
    }
}

/// 单个字符的终端显示宽度（CJK 全角字符占 2 列）。
///
/// `format!` 的 `{:<40}` 按 `char` 计数，中文会因此整体偏左。启动横幅是
/// 操作者第一眼看到的东西，歪掉的框会让人怀疑输出本身有问题。
fn char_width(c: char) -> usize {
    let cp = c as u32;
    let wide = matches!(
        cp,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
    );
    if wide {
        2
    } else {
        1
    }
}

/// 按显示宽度截断并补齐，输出一行带边框的横幅内容。
fn print_box_row(text: &str, inner: usize) {
    let body = inner.saturating_sub(2);
    let mut out = String::new();
    let mut w = 0usize;
    for c in text.chars() {
        let cw = char_width(c);
        if w + cw > body {
            break;
        }
        out.push(c);
        w += cw;
    }
    println!("║ {}{} ║", out, " ".repeat(body - w));
}
