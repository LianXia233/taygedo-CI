mod api;
mod constants;
mod crypto;
mod login;
mod models;
mod protocol;
mod runner;
mod scheduler;
mod service;
mod session;
mod store;
mod time;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let data_dir = std::env::var("TAYGEDO_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let listen = std::env::var("TAYGEDO_LISTEN").unwrap_or_else(|_| "0.0.0.0:8787".to_string());

    let state = service::AppState::new(PathBuf::from(&data_dir));
    let bind_addr: SocketAddr = listen.parse().expect("TAYGEDO_LISTEN 格式应为 host:port");

    state.push_log(
        "info",
        format!("塔吉多自动签到 Rust 版已启动，数据目录：{}", data_dir),
    );

    // 启动每日定时调度
    scheduler::spawn(state.clone());

    let app = web::router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .expect("绑定端口失败");

    // 计算可点击的访问地址：若绑定 0.0.0.0 则展示 localhost/127.0.0.1
    let display_url = if bind_addr.ip().is_unspecified() {
        format!("http://127.0.0.1:{}", bind_addr.port())
    } else {
        format!("http://{}", bind_addr)
    };
    let listen_note = if bind_addr.ip().is_unspecified() {
        format!("监听所有接口 0.0.0.0:{}", bind_addr.port())
    } else {
        format!("监听 {}", bind_addr)
    };

    let auth_note = if state.no_auth {
        "免鉴权模式 (内网放行)".to_string()
    } else {
        "账号密码登录".to_string()
    };

    // 读取加密策略用于横幅展示（不暴露任何密钥材料）
    let crypto_policy = state.config.read().await.crypto_policy.clone();
    let crypto_note = match crypto_policy.as_str() {
        "always" => "应用层加密: 强制 (X25519+AES-256-GCM)",
        "never" => "应用层加密: 已关闭",
        _ => "应用层加密: 内网明文/外网强制",
    };

    println!("╔════════════════════════════════════════════╗");
    println!("║  塔吉多自动签到 (Rust) 已启动               ║");
    println!("║                                              ║");
    println!("║  访问地址: {:<34} ║", display_url);
    println!("║  {}{:.<39}║", listen_note, "");
    println!("║  鉴权: {:<38} ║", auth_note);
    println!("║  {}{:.<39}║", crypto_note, "");
    println!("║  数据目录: {:<35} ║", data_dir);
    println!("╚════════════════════════════════════════════╝");

    // 初始随机口令：仅在 stdout 出现一次，**不写入日志缓冲区**（日志可经 API 读取）。
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
    // 服务器 / OpenWrt / Docker 无桌面环境，仅在 Windows 下执行，避免无意义弹窗。
    #[cfg(target_os = "windows")]
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
                        format!("自动打开 WebUI 失败（退出码 {}），请手动访问 {}", code, open_url),
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
