mod api;
mod constants;
mod crypto;
mod login;
mod models;
mod protocol;
mod runner;
mod scheduler;
mod service;
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
        "免鉴权模式 (无需登录)".to_string()
    } else {
        "默认账号: admin / admin".to_string()
    };

    println!("╔════════════════════════════════════════════╗");
    println!("║  塔吉多自动签到 (Rust) 已启动               ║");
    println!("║                                              ║");
    println!("║  访问地址: {:<34} ║", display_url);
    println!("║  {}{:.<39}║", listen_note, "");
    println!("║  鉴权: {:<38} ║", auth_note);
    println!("║  数据目录: {:<35} ║", data_dir);
    println!("╚════════════════════════════════════════════╝");

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

    axum::serve(listener, app).await.expect("服务运行失败");
}
