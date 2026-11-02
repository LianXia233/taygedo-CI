mod api;
mod constants;
mod crypto;
mod crypto_storage;
mod login;
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

    // 监听地址与访问方式说明。
    //
    // 本服务**不提供内置 WebUI**：独立部署时它是纯 API 端口，OpenWrt 上入口
    // 是 LuCI 菜单（数据由 rpcd/ucode 内部通道取回）。因此不再打印任何
    // "浏览器打开 :8787" 形式的地址，避免误导。
    let listen_note = if bind_addr.ip().is_unspecified() {
        format!("监听 0.0.0.0:{}（仅 API，无网页界面）", bind_addr.port())
    } else {
        format!("监听 {}（仅 API，无网页界面）", bind_addr)
    };

    let auth_note = if service::luci_session_required() {
        "LuCI 会话（rpcd/ucode 内部通道）"
    } else {
        "受信来源（回环/内网白名单）"
    };

    // 读取加密策略用于横幅展示（不暴露任何密钥材料）
    let crypto_policy = state.config.read().await.crypto_policy.clone();
    let crypto_note = match crypto_policy.as_str() {
        "always" => "应用层加密: 强制 (v2)",
        "never" => "应用层加密: 已关闭 (明文传输)",
        _ => "应用层加密: 内网明文/外网强制",
    };

    println!("╔════════════════════════════════════════════╗");
    println!("║  塔吉多自动签到 (Rust) 已启动               ║");
    println!("║                                              ║");
    println!("║  {}{:.<39}║", listen_note, "");
    println!("║  鉴权: {:<38} ║", auth_note);
    println!("║  {}{:.<39}║", crypto_note, "");
    println!("║  数据目录: {:<35} ║", data_dir);
    println!(
        "║  配置加密: XChaCha20-Poly1305 / {:<12} ║",
        state.store.key_protection().as_str()
    );
    println!("║  Key ID  : {:<35} ║", state.store.key_id_hex());
    // 传输层身份指纹：供**带外核对**（TOFU 首次信任窗口的唯一缓解手段）。
    // 打印它是安全的：指纹是公钥的哈希，不是密钥材料。
    println!("║  身份指纹: {:<35} ║", state.crypto.identity_fingerprint());
    println!("╚════════════════════════════════════════════╝");

    // OpenWrt 集成模式：说明入口与鉴权归属，避免使用者继续找 WebUI 端口。
    if service::is_openwrt() {
        println!();
        println!("[OpenWrt 集成模式] 无内置 WebUI、无浏览器侧加密前端。");
        println!("  唯一入口：LuCI 菜单「服务 → 塔吉多签到」");
        println!("  数据通道：rpcd/ucode 在服务端调用本机回环接口，浏览器不直连本端口");
        println!("  鉴权：由 LuCI/ubus 会话负责（本服务只接受回环或受信内网来源）");
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
