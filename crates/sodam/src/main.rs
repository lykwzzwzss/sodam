//! SodaM——GPUI 原生客户端入口。
//
// 发布版不分配控制台窗口（双击/快捷方式启动不再弹 terminal）；
// debug 构建保留终端，便于查看 eprintln 日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
// 搜索框的按键分发与输入法桥接，见 app_text.rs。
mod app_text;
// 当前曲目封面：取图、缓存、算整页色板，见 artwork.rs。
mod artwork;
// 本地音乐库：选文件夹、扫描、离线播放，见 app_local.rs。
mod app_local;
// 列表滚轮平滑滚动，见 app_scroll.rs。
mod app_scroll;
// 托盘：Linux 走 ksni/SNI，macOS 走 NSStatusItem，见 tray.rs。
mod tray;
mod ui;
mod views;
// 系统媒体控制（媒体键 / 桌面媒体挂件），见 media.rs。
mod media;
// 播放控制快捷键，见 shortcuts.rs。
mod shortcuts;
// 单实例防护：第二次启动只把已有窗口提到前台，见 single_instance.rs。
mod single_instance;
// 任务栏快捷播控（Windows 任务栏播放条），见 taskbar_player/mod.rs。
mod taskbar_player;
// macOS 菜单栏与标准快捷键（⌘Q/⌘M/⌘W），见 menus.rs。
#[cfg(target_os = "macos")]
mod menus;

use gpui::{
    px, size, App, AppContext as _, Bounds, Entity, TitlebarOptions, WindowBounds, WindowOptions,
};
use std::sync::Arc;

/// 启动窗口的**客户区物理像素**（2560×1600@150% 下约 82% × 87.5% 屏）。
/// gpui 内部一律逻辑像素；`main_window_options` 按显示器缩放比例换算，换显示器/
/// 缩放比例时客户区仍是同一块物理大小。
const DEFAULT_SIZE: (f32, f32) = (2100.0, 1400.0);
const MIN_SIZE: (f32, f32) = (1120.0, 720.0);

fn main_window_options(cx: &mut App) -> WindowOptions {
    // 目标客户区按**物理像素** 2100×1400 固定（2560×1600@150% 下的约 82% × 87.5%）。
    // gpui 的窗口 bounds 是逻辑像素；按显示器缩放比例换算：1.5 倍屏 → 1400×933
    // 逻辑 → 设备 2100×1400。屏比 125% 下同样换算后仍是同一块物理大小。
    #[cfg(target_os = "windows")]
    let scale = (unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() } as f32 / 96.0).max(1.0);
    #[cfg(not(target_os = "windows"))]
    let scale = 1.0;
    let target_logical = size(px(DEFAULT_SIZE.0 / scale), px(DEFAULT_SIZE.1 / scale));
    let bounds = Bounds::centered(None, target_logical, cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        titlebar: Some(TitlebarOptions {
            title: Some("SodaM".into()),
            // macOS 隐藏系统标题栏（红绿灯悬浮）；Windows 同样隐藏，
            // 由 ui::titlebar 自绘拖拽区与最小化/最大化/关闭按钮。
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            appears_transparent: true,
            ..Default::default()
        }),
        app_id: Some("SodaM".into()),
        icon: Some(Arc::new(
            image::load_from_memory(include_bytes!("../assets/brand/sodam-logo-tray.png"))
                .expect("内置应用图标应为有效 PNG")
                .to_rgba8(),
        )),
        ..Default::default()
    }
}

/// 把主窗口从最小化/隐藏状态拉回来。
///
/// gpui 的 `activate_window()` 只处理最小化（`IsIconic`）；窗口被
/// `SW_HIDE` 隐藏时不会重新显示，这里补一次原生 `SW_RESTORE`。
fn activate_main_window(window: &mut gpui::Window) {
    window.activate_window();
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = native_handle(window) {
        // 用 SW_SHOW 而不是 SW_RESTORE：最小化已经由 activate_window 还原，
        // 这里只负责把隐藏窗口显示出来，不能把最大化的窗口退回普通尺寸。
        const SW_SHOW: i32 = 5;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn ShowWindow(hwnd: *mut std::ffi::c_void, cmd: i32) -> i32;
}

/// Windows 上取窗口的原生 HWND（系统媒体控件挂在窗口上）；其它平台没有这个概念。
#[cfg(target_os = "windows")]
fn native_handle(window: &gpui::Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // 必须显式写 trait 名：gpui 的 `Window` 自带一个同名的 `window_handle()`（返回
    // `AnyWindowHandle`），方法语法会优先命中那个，拿不到原生句柄。
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn native_handle(_window: &gpui::Window) -> Option<*mut std::ffi::c_void> {
    None
}

/// 托盘事件循环：把命令转发为应用动作，并周期同步播放状态到 `sync`。
/// 双平台共用；平台差异只在 `sync` 闭包（见 tray.rs）。
fn start_tray_service(
    rx: std::sync::mpsc::Receiver<tray::TrayCommand>,
    app: Entity<app::Root>,
    cx: &mut App,
    mut sync: impl FnMut(tray::TrayState) + 'static,
) {
    cx.spawn(async move |cx| {
        let mut tick = 0u32;
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;
            while let Ok(command) = rx.try_recv() {
                match command {
                    tray::TrayCommand::Show => cx.update(|cx| show_window(cx, &app)),
                    tray::TrayCommand::Toggle => {
                        cx.update(|cx| app.update(cx, |root, cx| root.toggle_play(cx)))
                    }
                    tray::TrayCommand::Previous => {
                        cx.update(|cx| app.update(cx, |root, cx| root.prev_track(cx)))
                    }
                    tray::TrayCommand::Next => {
                        cx.update(|cx| app.update(cx, |root, cx| root.next_track(cx)))
                    }
                    tray::TrayCommand::Quit => cx.update(|cx| cx.quit()),
                };
            }

            // 系统媒体控件（媒体键 / 桌面挂件按钮）的请求 → 播放动作。
            for command in cx.update(|cx| media::drain_commands(cx)) {
                cx.update(|cx| {
                    app.update(cx, |root, cx| {
                        let playing = root.engine.snapshot().playing;
                        match command {
                            media::Command::Play => {
                                if !playing {
                                    root.toggle_play(cx);
                                }
                            }
                            media::Command::Pause => {
                                if playing {
                                    root.toggle_play(cx);
                                }
                            }
                            media::Command::Toggle => root.toggle_play(cx),
                            media::Command::Next => root.next_track(cx),
                            media::Command::Previous => root.prev_track(cx),
                        }
                    })
                });
            }

            tick += 1;
            if tick % 5 == 0 {
                let now = cx.update(|cx| {
                    app.update(cx, |root, _cx| {
                        let snapshot = root.engine.snapshot();
                        let track = root.queue.current();
                        // 托盘与系统媒体面板都读本地 96px 缓存封面（没有就没有）。
                        let cover_path = root.player_bar_cover();
                        let title = if snapshot.track_id.is_empty() {
                            "SodaM".to_string()
                        } else {
                            snapshot.title.clone()
                        };
                        let subtitle = if snapshot.track_id.is_empty() {
                            root.tr("就绪").to_string()
                        } else {
                            track.map(|track| track.artist.clone()).unwrap_or_default()
                        };
                        sync(tray::TrayState {
                            language: root.language,
                            title,
                            subtitle,
                            playing: snapshot.playing,
                            has_track: !snapshot.track_id.is_empty(),
                            cover_path: cover_path.clone(),
                        });
                        // 顺手发布给系统媒体面板（内容没变时 publish 内部会跳过）。
                        media::NowPlaying {
                            track_id: snapshot.track_id.clone(),
                            title: snapshot.title.clone(),
                            artist: track.map(|t| t.artist.clone()).unwrap_or_default(),
                            album: track.map(|t| t.album.clone()).unwrap_or_default(),
                            cover_path,
                            duration_seconds: snapshot.duration_seconds,
                            position_seconds: snapshot.position_seconds,
                            playing: snapshot.playing,
                        }
                    })
                });
                cx.update(|cx| media::publish(cx, &now));
            }
        }
    })
    .detach();
}

/// 打开或激活主窗口；由托盘、二次启动和任务栏播放条调用。
///
/// 不能取 `cx.windows().first()`：任务栏播放条也是窗口，主窗口最小化或关闭后
/// 它可能排在第一位，结果只激活了播放条。
fn show_window(cx: &mut App, app: &Entity<app::Root>) {
    // 旧窗口关闭时鼠标停在关闭按钮上，hover 出去的信号不会再来了；
    // 不清理的话新窗口一打开关闭按钮还是红的。
    app.update(cx, |root, cx| {
        root.hovers.clear();
        cx.notify();
    });
    let mut activated = false;
    if let Some(window) = cx
        .windows()
        .into_iter()
        .find_map(|handle| handle.downcast::<app::Root>())
    {
        match window.update(cx, |_, window, _| activate_main_window(window)) {
            Ok(()) => activated = true,
            // 句柄还在列表里但窗口已经销毁：不要静默当成功，下面直接重建。
            Err(error) => sodam_core::log_outlet::log_event(
                sodam_core::log_outlet::LogLevel::Warn,
                sodam_core::log_outlet::LogTopic::App,
                "main_window_activate_failed",
                &[("detail", &format!("{error:?}"))],
            ),
        }
    }
    if !activated {
        let app = app.clone();
        let options = main_window_options(cx);
        match cx.open_window(options, |window, cx| {
            // 新窗口有自己的 HWND：系统媒体控制要重新挂上去，
            // 否则关掉旧窗口后媒体键/桌面挂件还指向已经销毁的句柄。
            media::attach(native_handle(window), cx);
            app
        }) {
            Ok(window) => {
                if let Err(error) = window.update(cx, |_, window, _| activate_main_window(window)) {
                    sodam_core::log_outlet::log_event(
                        sodam_core::log_outlet::LogLevel::Warn,
                        sodam_core::log_outlet::LogTopic::App,
                        "main_window_activate_failed",
                        &[("detail", &format!("{error:?}"))],
                    );
                }
                sodam_core::log_outlet::log_event(
                    sodam_core::log_outlet::LogLevel::Info,
                    sodam_core::log_outlet::LogTopic::App,
                    "main_window_reopened",
                    &[],
                );
            }
            Err(error) => sodam_core::log_outlet::log_event(
                sodam_core::log_outlet::LogLevel::Warn,
                sodam_core::log_outlet::LogTopic::App,
                "main_window_reopen_failed",
                &[("detail", &format!("{error:?}"))],
            ),
        }
    }
    cx.activate(true);
}

/// 第二个实例启动时发来的「把窗口提到前台」请求。
///
/// socket 监听在独立线程，这里用轮询把信号搬到 UI 线程 —— 不能在渲染线程上
/// 阻塞 `recv()`，200ms 的延迟对这个操作完全够用。
fn start_single_instance_service(
    activations: std::sync::mpsc::Receiver<()>,
    app: Entity<app::Root>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(200))
            .await;
        while activations.try_recv().is_ok() {
            cx.update(|cx| show_window(cx, &app));
        }
    })
    .detach();
}

fn main() {
    // 单实例：第二次启动不再开第二个窗口，只把已在跑的窗口提到前台。
    // Secondary 时已经代为请求前台，本进程直接退出（静默，符合 issue #14）。
    let instance = single_instance::acquire();
    let single_instance::Instance::Primary {
        _guard,
        activations,
    } = instance
    else {
        return;
    };

    sodam_core::log_outlet::init();

    gpui_platform::application()
        .with_assets(ui::icons::Assets)
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(|cx: &mut App| {
            let app = cx.new(app::Root::new);
            // macOS：装好菜单栏与 ⌘Q/⌘M/⌘W（菜单标题跟随当前语言）。
            #[cfg(target_os = "macos")]
            {
                let language = app.read(cx).language;
                menus::install(cx, language);
            }
            // 播放快捷键：应用内三键（配置里可改）。
            let settings = app.read(cx).settings.clone();
            shortcuts::install(cx, &settings, app.clone());
            let options = main_window_options(cx);
            cx.open_window(options, |window, cx| {
                // 系统媒体控制要等窗口存在：Windows 的 SMTC 挂在 HWND 上。
                media::attach(native_handle(window), cx);
                app.clone()
            })
            .expect("打开主窗口失败");

            {
                let enabled = app.read(cx).settings.taskbar_player;
                taskbar_player::start_if_enabled(cx, app.clone(), enabled);
            }

            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                let (tray_tx, tray_rx) = std::sync::mpsc::channel();

                // 平台各自的托盘创建 + 状态同步闭包。
                #[cfg(target_os = "linux")]
                let sync = {
                    let tray_service = ksni::TrayService::new(tray::linux::SodaTray::new(
                        tray_tx,
                        crate::ui::i18n::Language::system_locale(),
                    ));
                    let tray_handle = tray_service.handle();
                    tray_service.spawn();
                    move |state: tray::TrayState| {
                        // ksni 0.2 的 Handle::update 返回 ()，直接调用。
                        tray_handle.update(|tray| {
                            tray.language = state.language;
                            tray.title = state.title;
                            tray.subtitle = state.subtitle;
                            tray.playing = state.playing;
                            // 之前漏了这两个：菜单顶部「正在播放」那行由 has_track 决定，
                            // 不同步的话播着歌也一直显示「未在播放」。
                            tray.has_track = state.has_track;
                            tray.cover_path = state.cover_path.clone();
                        });
                    }
                };
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                let sync =
                    tray::create_status_item(tray_tx, crate::ui::i18n::Language::system_locale());

                start_tray_service(tray_rx, app.clone(), cx, sync);
            }

            start_single_instance_service(activations, app.clone(), cx);

            cx.activate(true);
        });
}
