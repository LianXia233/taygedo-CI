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

    println!("===============================================");
    println!("  塔吉多自动签到 (Rust) 已启动");
    println!("  访问地址: http://{}", bind_addr);
    println!("  数据目录: {}", data_dir);
    println!("===============================================");

    axum::serve(listener, app).await.expect("服务运行失败");
}
