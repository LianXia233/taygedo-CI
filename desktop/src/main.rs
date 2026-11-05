//! taygedo 桌面单包 composition root。
//!
//! 结构：**一个进程**同时承担两件事 —— 内嵌 axum 服务（本机回环）+ GPUI 原生界面。
//!
//! 为什么不是「客户端 + 同目录服务 exe」的双进程：子进程会在客户端退出后继续
//! 常驻，Windows 因此锁住 `taygedo-rs.exe` 的映像文件，用户删除安装目录会失败。
//! 单进程形态下，关窗口即进程退出，所有文件句柄随之释放。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod client;
mod state;
mod theme;
mod views;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use client::ApiClient;
use gpui::{
    App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};
use gpui_component::Root;
use state::{AppState, Connection};
use views::RootView;

#[cfg(target_os = "macos")]
fn desktop_application() -> gpui::Application {
    gpui::Application::new_inaccessible(gpui_platform::current_platform(false))
}

#[cfg(not(target_os = "macos"))]
fn desktop_application() -> gpui::Application {
    gpui_platform::application()
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // reqwest 依赖 tokio 的 IO/time 驱动；内嵌服务也跑在同一个运行时上。
    // Application::run 的启动闭包是 FnOnce，闭包返回即释放其中的局部变量，
    // 因此 Runtime 必须提升为 'static，否则内嵌服务与刷新任务会一起被取消。
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            log::error!("无法创建 tokio 运行时：{error}");
            std::process::exit(1);
        }
    };
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(runtime));
    let handle = runtime.handle().clone();

    // 显式指定基址时不再启动内嵌服务：用户可能刻意指向另一个已在运行的实例，
    // 或把端口让给别的程序。未指定则用编译期回环地址常量。
    let override_base = std::env::var("TAYGEDO_API_BASE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let listen: SocketAddr = taygedo::DESKTOP_LISTEN
        .parse()
        .expect("DESKTOP_LISTEN 常量必须是合法 SocketAddr");

    let mut base = override_base
        .clone()
        .unwrap_or_else(|| client::DEFAULT_API_BASE.to_string());
    let mut service: Option<taygedo::ServiceHandle> = None;
    let mut startup_error: Option<String> = None;

    if override_base.is_some() {
        log::info!("按 TAYGEDO_API_BASE 连接外部实例：{base}");
    } else {
        // 已有实例在跑（例如用户开了第二个窗口）时直接复用，不抢端口。
        let reuse = ApiClient::new(base.clone())
            .map(|probe| runtime.block_on(probe.meta()).is_ok())
            .unwrap_or(false);

        if reuse {
            log::info!("检测到已在运行的实例，复用 {base}");
        } else {
            match runtime.block_on(taygedo::start(listen, resolve_data_dir())) {
                Ok(started) => {
                    base = format!("http://{}", started.listen);
                    log::info!("内嵌服务已启动：{base}");
                    service = Some(started);
                }
                Err(message) => {
                    startup_error = Some(format!(
                        "{message}\n数据目录不可写或端口被占用时会出现该错误；关闭其他实例后重试。"
                    ));
                }
            }
        }
    }

    let client = match ApiClient::new(base.clone()) {
        Ok(client) => client,
        Err(error) => {
            log::error!("无法创建 API 客户端：{error}");
            std::process::exit(1);
        }
    };

    let app = desktop_application().with_assets(gpui_component_assets::Assets);
    app.run(move |cx: &mut App| {
        gpui_component::init(cx);
        gpui_component::set_locale("zh-CN");

        let state = cx.new(|_| {
            AppState::new(
                client.clone(),
                handle.clone(),
                base.clone(),
                startup_error.clone(),
            )
        });
        state.update(cx, |state, cx| {
            state.start_polling(cx);
            if !matches!(&state.connection, Connection::Failed(_)) {
                state.bootstrap(cx);
            }
        });

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(90.), px(50.)),
                size: size(px(1180.), px(840.)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("塔吉多自动签到".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            ..Default::default()
        };
        if let Err(error) = cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| RootView::new(state.clone(), window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        }) {
            log::error!("创建主窗口失败：{error:#}");
        }
    });

    // 窗口全部关闭后 app.run 返回：此处主动关闭内嵌服务，保证进程干净退出，
    // 不留任何后台线程或监听端口。
    if let Some(service) = service {
        service.shutdown();
        // 给优雅关闭留出极短窗口，然后直接退出进程（避免运行时被 leak 后卡住）。
        std::thread::sleep(Duration::from_millis(80));
    }
}

/// 数据目录：默认放在**可执行文件同目录的 `data/`**（绿色版行为），
/// 可用 `TAYGEDO_DATA_DIR` 覆盖。
///
/// 不依赖当前工作目录：从快捷方式或开始菜单启动时 CWD 往往不是 exe 所在目录，
/// 用 CWD 会把数据写进 System32 之类的意外位置。
fn resolve_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TAYGEDO_DATA_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    match std::env::current_exe() {
        Ok(exe) => match exe.parent() {
            Some(dir) => dir.join("data"),
            None => PathBuf::from("data"),
        },
        Err(_) => PathBuf::from("data"),
    }
}
