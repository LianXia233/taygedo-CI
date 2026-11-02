// 构建形态在编译期确定，运行期不再做任何探测。
//
// 两者互斥：一个二进制不可能同时"自带 WebUI"和"由 LuCI 独占入口"。
// 同时开启会让路由与鉴权出现二义性；一个都不开则没有任何访问控制路径。
// 因此这里用编译错误把配置错误挡在构建阶段，而不是留到运行期才暴露。
#[cfg(all(feature = "webui", feature = "openwrt"))]
compile_error!(
    "`webui` 与 `openwrt` 是互斥形态：一次构建只能产出一种。\
     请用 --no-default-features --features openwrt 构建 OpenWrt 版本"
);

#[cfg(not(any(feature = "webui", feature = "openwrt")))]
compile_error!("必须显式选择构建形态：`webui`（Windows/Debian/Docker）或 `openwrt`（LuCI 集成）");

mod api;
mod constants;
mod crypto;
mod crypto_storage;
mod login;
#[cfg(feature = "openwrt")]
mod luci_session;
mod models;
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

#[tokio::main]
async fn main() {
    let data_dir = std::env::var("TAYGEDO_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let listen = std::env::var("TAYGEDO_LISTEN").unwrap_or_else(|_| "0.0.0.0:8787".to_string());

    // 运维子命令：密钥轮换。必须在建立 AppState 之前单独处理，
    // 因为它要求"先取锁 → 重新加密全部文件 → 提升新密钥"的独占流程。
    if std::env::args().any(|a| a == "--rotate-storage-key") {
        std::process::exit(rotate_storage_key_cli(&data_dir));
    }

    // 存储层初始化失败 = 不可启动。绝不带着"没有密钥/没迁移完"的状态
    // 继续对外提供服务——那会在下一次写入时静默覆盖掉用户数据。
    let state = match service::AppState::new(PathBuf::from(&data_dir)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[fatal] 启动失败：{e}");
            eprintln!("[fatal] 数据目录：{data_dir}");
            eprintln!(
                "[fatal] 请勿删除数据目录中的密文文件；恢复步骤见 README「加密存储 / 故障恢复」。"
            );
            std::process::exit(1);
        }
    };
    let bind_addr: SocketAddr = listen.parse().expect("TAYGEDO_LISTEN 格式应为 host:port");

    state.push_log(
        "info",
        format!(
            "塔吉多自动签到 Rust 版已启动，数据目录：{}",
            state.store.data_dir().display()
        ),
    );

    // 启动每日定时调度
    scheduler::spawn(state.clone());

    let app = web::router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .expect("绑定端口失败");

    // ---- 启动横幅 ----
    //
    // 形态在**编译期**已确定，这里只按 feature 选择文案，不含任何运行期探测。
    // OpenWrt 产物里没有 WebUI 代码，因此连"浏览器打开 :8787"这句话都不会出现。
    #[cfg(feature = "webui")]
    let display_url = if bind_addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{}", bind_addr.port())
    } else {
        format!("http://{}", bind_addr)
    };
    #[cfg(feature = "openwrt")]
    let display_url = "LuCI 菜单：服务 → 塔吉多签到".to_string();

    let listen_note = if bind_addr.ip().is_unspecified() {
        format!("监听所有接口 0.0.0.0:{}", bind_addr.port())
    } else {
        format!("监听 {}", bind_addr)
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

    #[cfg(feature = "webui")]
    let crypto_note = match state.config.read().await.crypto_policy.as_str() {
        "always" => "应用层加密: 强制 (v2)",
        "never" => "应用层加密: 已关闭 (明文传输)",
        _ => "应用层加密: 内网明文/外网强制",
    };
    // OpenWrt 形态不存在浏览器侧消费者：不挂加密信封中间件，也没有握手端点。
    #[cfg(feature = "openwrt")]
    let crypto_note = "应用层加密: 不适用 (内部通道)";

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
    // 仅 webui 形态存在握手端点；OpenWrt 走 LuCI 内部通道，没有 TOFU 场景，
    // 打印一个无消费者的指纹只会让人误以为需要核对。
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

    // Windows 桌面端：启动后自动用默认浏览器打开 WebUI。
    // 服务器 / Docker 无桌面环境，仅在 Windows 下执行，避免无意义弹窗。
    #[cfg(all(feature = "webui", target_os = "windows"))]
    {
        let open_url = display_url.clone();
        tokio::spawn(async move {
            // 稍等，确保 axum 已开始监听，避免浏览器首请求连不上
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            let status = std::process::Command::new("cmd")
                .args(["/C", "start", "", open_url.as_str()])
                .status();
            match status {
                Ok(code) if code.success() => {
                    state.push_log("info", format!("已自动打开 WebUI：{}", open_url));
                }
                Ok(code) => {
                    state.push_log(
                        "warn",
                        format!(
                            "自动打开 WebUI 失败（退出码 {}），请手动访问 {}",
                            code, open_url
                        ),
                    );
                }
                Err(e) => {
                    state.push_log(
                        "warn",
                        format!("自动打开 WebUI 失败（{}），请手动访问 {}", e, open_url),
                    );
                }
            }
        });
    }

    // 通过 into_make_service_with_connect_info 注入来源地址，
    // 供免鉴权 LAN 判定与握手限速使用。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("服务运行失败");
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
