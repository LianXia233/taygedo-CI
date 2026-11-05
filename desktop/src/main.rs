//! taygedo 桌面客户端 composition root。
//!
//! Windows 端以 GPUI 原生界面替代 WebUI：打开即连本机回环 REST API，免鉴权。
//! 启动前探测 /api/meta；不可达时按 current_exe 同目录拉起服务子进程。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod client;
mod state;
mod views;

use std::path::PathBuf;
use std::process::Command;
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let base = std::env::var("TAYGEDO_API_BASE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| client::DEFAULT_API_BASE.to_string());

    let client = match ApiClient::new(base.clone()) {
        Ok(client) => client,
        Err(error) => {
            log::error!("无法创建 API 客户端：{error}");
            std::process::exit(1);
        }
    };

    // reqwest 依赖 tokio 的 IO/time 驱动，独立 runtime 贯穿整个应用生命周期。
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
    // Application::run 的启动闭包是 FnOnce(&mut App)，闭包返回即释放其中的局部变量。
    // 若把 Runtime 留在闭包里，AppState::execute() 经 handle.spawn() 投递的任务会被取消，
    // 状态栏会永远卡在「正在刷新…」。因此把 Runtime 提升为 'static，贯穿进程生命周期。
    let runtime: &'static tokio::runtime::Runtime = Box::leak(Box::new(runtime));
    let handle = runtime.handle().clone();

    // 启动探活：不可达则拉起同目录服务；找不到服务时在 UI 明确报错，不静默失败。
    let reachable = runtime.block_on(client.meta()).is_ok();
    let startup_error: Option<String> = if reachable {
        None
    } else {
        match spawn_service() {
            Ok(()) => {
                if wait_until_ready(runtime, &client) {
                    None
                } else {
                    Some(
                        "已拉起服务子进程，但在限定时间内 /api/meta 仍不可达。请确认服务以 desktop 形态启动、8787 端口未被占用。".to_string(),
                    )
                }
            }
            Err(message) => Some(message),
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
            state.start_auto_refresh(cx);
            if !matches!(&state.connection, Connection::Failed(_)) {
                state.bootstrap(cx);
            }
        });

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(80.), px(60.)),
                size: size(px(1060.), px(780.)),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("塔吉多签到 · 桌面客户端".into()),
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
}

/// 按 `current_exe()` 同目录查找并拉起服务；Windows 子进程设 CREATE_NO_WINDOW。
fn spawn_service() -> Result<(), String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("无法定位当前可执行文件：{error}"))?;
    let dir: PathBuf = exe
        .parent()
        .ok_or_else(|| "无法定位可执行文件所在目录".to_string())?
        .to_path_buf();

    // 先做平台检测再分支：Windows 与 Unix 的可执行文件名不同。
    let name = if cfg!(target_os = "windows") {
        "taygedo-rs.exe"
    } else {
        "taygedo-rs"
    };
    let service = dir.join(name);
    if !service.is_file() {
        return Err(format!(
            "本机回环 API 不可达，且在 {} 未找到服务可执行文件 {name}。\n请将 taygedo-rs（Windows 为 taygedo-rs.exe）放在本程序同目录，或先手动启动服务；也可设置 TAYGEDO_API_BASE 指向已运行的本机回环服务。",
            dir.display()
        ));
    }

    let mut command = Command::new(&service);
    command
        .env("TAYGEDO_LISTEN", "127.0.0.1:8787")
        .env("TAYGEDO_CRYPTO_POLICY", "never");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows 平台无控制台窗口标志：空操作。
    }

    match command.spawn() {
        Ok(child) => {
            log::info!(
                "已拉起服务子进程：{} (pid {})",
                service.display(),
                child.id()
            );
            // 服务独立运行：不等待、不随客户端句柄释放而终止。
            drop(child);
            Ok(())
        }
        Err(error) => Err(format!(
            "找到服务可执行文件 {}，但拉起失败：{error}",
            service.display()
        )),
    }
}

/// 拉起服务后轮询 /api/meta，最多等待约 10 秒。
fn wait_until_ready(runtime: &tokio::runtime::Runtime, client: &ApiClient) -> bool {
    for _ in 0..20 {
        let ready = runtime.block_on(async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            client.meta().await.is_ok()
        });
        if ready {
            return true;
        }
    }
    false
}
