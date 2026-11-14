//! Windows 宿主层：把 gpui 的无边框置顶窗口摆到任务栏空闲区，并跟随系统状态显隐。
//!
//! 位置 = UIA 占用探测 + 托盘硬边界 → 空闲区间 → 落位；任务栏可见性、全屏和
//! 布局变化由 WinEvent 钩子触发重探，5s 兜底。任何一步判定不可靠就隐藏。
//!
//! 坐标约定：宿主层内部一律**物理像素**（进程 PerMonitorV2，`GetWindowRect`
//! 不做 DPI 虚拟化），只在交给 gpui 的窗口 bounds 处换算成逻辑像素。

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use gpui::{
    point, px, size, App, AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions,
};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, SetLastError, ERROR_SUCCESS, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    RegisterWindowMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, GWLP_HWNDPARENT, GWL_EXSTYLE,
    GW_HWNDPREV, HWND_TOPMOST, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNA, WINDOW_EX_STYLE, WINEVENT_OUTOFCONTEXT, WM_DISPLAYCHANGE, WM_MOUSEACTIVATE,
    WM_SETTINGCHANGE, WS_EX_NOACTIVATE,
};

use super::geometry;
use super::probe;
use super::view::{TaskbarPlayerView, BAR_WIDTH};
use super::{probe_is_current, state_for, Rect, TaskbarHostState};
use sodam_core::log_outlet::{log_event, LogLevel, LogTopic};

/// 子类标识。
const SUBCLASS_ID: usize = 0x5344_4D50;
/// 信息区固定 10 个字宽，空间不够就隐藏，不再压缩布局。
const BAR_MIN_WIDTH: f32 = BAR_WIDTH;
/// 与任务栏上下边缘的留白（逻辑像素）。
const BAR_INSET: f32 = 4.0;
/// 与托盘（含「显示隐藏的图标」这类展开键）保持的间距，逻辑像素。
const TRAY_MARGIN: f32 = 34.0;
/// 托盘区控件已被 UIA 读到时用的小边距（真实边界已知）。
const TRAY_MARGIN_CLOSE: f32 = 10.0;
/// 托盘外沿左侧多宽范围内出现控件，就算「托盘区已读到」。
const CHEVRON_ZONE: f32 = 64.0;
/// 布局事件合并窗口：一次重排里的多个 WinEvent 只触发一次探测。
const PROBE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);
/// 淡出后再隐藏窗口的等待（略大于视图的 250ms 淡出）。
const FADE_OUT: std::time::Duration = std::time::Duration::from_millis(280);
/// 兜底重探周期。
const PROBE_FALLBACK: std::time::Duration = std::time::Duration::from_secs(5);
const SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
/// 探测线程异常退出（结果永远不回）时，别让 `probe_pending` 把后续探测锁死。
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// 前台切换事件的计数：回调里只递增，真正处理放在 tick 里。
static FOREGROUND_EVENTS: AtomicU32 = AtomicU32::new(0);
/// 任务栏相关事件（位置/显隐）的计数。
static TASKBAR_EVENTS: AtomicU32 = AtomicU32::new(0);
/// `TaskbarCreated` 注册消息号；子类过程里只能读原子量（见 `subclass` 注释）。
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static EXPLORER_RESTARTS: AtomicU32 = AtomicU32::new(0);
/// 宿主实例序号：关闭后立刻重开时，旧同步循环看到序号变了就退出。
static HOST_EPOCH: AtomicU64 = AtomicU64::new(0);
static STARTING: AtomicBool = AtomicBool::new(false);

struct Host {
    hwnd: HWND,
    taskbar: HWND,
    epoch: u64,
    restarts: u32,
    taskbar_rect: Rect,
    bar: Rect,
    state: TaskbarHostState,
    occupied: Vec<Rect>,
    generation: u64,
    tray_reported: bool,
    has_space: bool,
    probe_pending: bool,
    view_shown: bool,
    dirty_since: Option<std::time::Instant>,
    last_probe: std::time::Instant,
    hooks: Vec<HWINEVENTHOOK>,
    last_foreground_events: u32,
    last_taskbar_events: u32,
    last_raise: std::time::Instant,
    window: Option<gpui::WindowHandle<TaskbarPlayerView>>,
    hide_at: Option<std::time::Instant>,
    misplaced: Option<Rect>,
}

impl Drop for Host {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
        unsafe {
            let _ = windows::Win32::UI::Shell::RemoveWindowSubclass(
                self.hwnd,
                Some(subclass),
                SUBCLASS_ID,
            );
        }
    }
}

/// 首次定位与后续重试共用后台 UIA 探测。关闭或重开时，旧代次的结果直接作废。
pub(crate) fn start(cx: &mut App, root: gpui::Entity<crate::app::Root>) {
    if HOST.with(|slot| slot.borrow().is_some()) || STARTING.swap(true, Ordering::Relaxed) {
        return;
    }
    let epoch = HOST_EPOCH.fetch_add(1, Ordering::Relaxed) + 1;
    cx.spawn(async move |cx| loop {
        let request = cx.update(|_| {
            primary_taskbar()
                .filter(|&taskbar| !covers_monitor(HWND::default(), taskbar))
                .and_then(|taskbar| {
                    let rect = window_rect(taskbar)?;
                    let hwnd = taskbar.0 as usize;
                    Some((hwnd, rect, probe::request(hwnd)?))
                })
        });
        if let Some((taskbar, rect, generation)) = request {
            let started = std::time::Instant::now();
            loop {
                cx.background_executor().timer(SYNC_INTERVAL).await;
                if HOST_EPOCH.load(Ordering::Relaxed) != epoch {
                    return;
                }
                if let Some(outcome) = probe::take_result()
                    .filter(|outcome| probe_is_current(outcome.generation(), generation))
                {
                    let attached = cx.update(|cx| {
                        let taskbar = HWND(taskbar as *mut c_void);
                        if primary_taskbar() != Some(taskbar)
                            || window_rect(taskbar) != Some(rect)
                            || covers_monitor(HWND::default(), taskbar)
                        {
                            return false;
                        }
                        let Some(plan) = plan(taskbar, outcome) else {
                            return false;
                        };
                        open_host(cx, root.clone(), plan, epoch)
                    });
                    if attached {
                        STARTING.store(false, Ordering::Relaxed);
                        return;
                    }
                    break;
                }
                if started.elapsed() >= PROBE_TIMEOUT {
                    log_event(LogLevel::Warn, LogTopic::App, "taskbar_probe_timeout", &[]);
                    break;
                }
            }
        }
        cx.background_executor().timer(PROBE_FALLBACK).await;
        if HOST_EPOCH.load(Ordering::Relaxed) != epoch {
            return;
        }
    })
    .detach();
}

fn primary_taskbar() -> Option<HWND> {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok()
}

fn open_host(cx: &mut App, root: gpui::Entity<crate::app::Root>, plan: Plan, epoch: u64) -> bool {
    let sync_root = root.clone();
    let scale = scale_of(plan.taskbar);
    let bounds = Bounds {
        origin: point(
            px(plan.bar.left as f32 / scale),
            px(plan.bar.top as f32 / scale),
        ),
        size: size(
            px(plan.bar.width() as f32 / scale),
            px(plan.bar.height() as f32 / scale),
        ),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // PopUp 在 Windows 上就是 WS_EX_TOOLWINDOW | WS_EX_TOPMOST，且没有系统标题栏。
        kind: WindowKind::PopUp,
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            ..Default::default()
        }),
        focus: false,
        // 窗口正常创建，内容先保持全透明，由同步循环确认显示状态后淡入。
        // 不能建隐藏窗口再 ShowWindow：gpui 不会为外部显示的窗口补首帧（实测会空白）。
        show: true,
        is_movable: false,
        is_resizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };

    let window = cx.open_window(options, move |window, cx| {
        match native_hwnd(window) {
            Some(hwnd) => attach(hwnd, plan),
            None => log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_probe_failed",
                &[("detail", "拿不到 HWND")],
            ),
        }
        cx.new(|cx| TaskbarPlayerView::new(cx, root.clone()))
    });
    let Ok(window) = window else {
        log_event(
            LogLevel::Warn,
            LogTopic::App,
            "taskbar_probe_failed",
            &[("detail", "open_window")],
        );
        return false;
    };

    // 首次探测已完成；记住窗口句柄，下一拍同步时再显示。
    let attached = HOST.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(host) = slot.as_mut() else {
            return false;
        };
        host.epoch = epoch;
        host.window = Some(window);
        true
    });
    if !attached {
        // 建窗成功但 attach 失败（拿不到 HWND）：别留一个空白窗口在任务栏上。
        log_event(LogLevel::Warn, LogTopic::App, "taskbar_attach_failed", &[]);
        if let Err(error) = window.update(cx, |_, window, _| window.remove_window()) {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_window_remove_failed",
                &[("detail", &format!("{error:?}"))],
            );
        }
        return false;
    }

    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(SYNC_INTERVAL).await;
        let alive = cx.update(|cx| {
            let needs_attach = HOST.with(|slot| {
                slot.borrow().as_ref().is_some_and(|host| {
                    host.epoch == epoch
                        && (primary_taskbar() != Some(host.taskbar)
                            || !unsafe { IsWindow(Some(host.hwnd)) }.as_bool()
                            || host.restarts != EXPLORER_RESTARTS.load(Ordering::Relaxed))
                })
            });
            if needs_attach {
                stop(cx);
                start(cx, sync_root.clone());
                return false;
            }
            sync(cx, epoch)
        });
        if !alive {
            break;
        }
    })
    .detach();
    true
}

struct Plan {
    taskbar: HWND,
    taskbar_rect: Rect,
    bar: Rect,
    occupied: Vec<Rect>,
    tray_reported: bool,
}

/// 算播放条矩形。位置来自 UIA 的占用矩形 + 托盘硬边界，任何一步不可靠就不建窗。
fn plan(taskbar: HWND, outcome: probe::Outcome) -> Option<Plan> {
    let taskbar_rect = window_rect(taskbar)?;
    let dpi = unsafe { GetDpiForWindow(taskbar) };
    let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };

    let occupied = match outcome {
        probe::Outcome::Valid { occupied, .. } => occupied,
        probe::Outcome::Uncertain { .. } => {
            log_probe_uncertain();
            return None;
        }
        probe::Outcome::Failed { detail, .. } => {
            log_probe_failed(&detail);
            return None;
        }
    };

    let (mut occupied, tray_reported) = occupied_with_tray(taskbar, taskbar_rect, scale, occupied);
    let bar = place_bar(taskbar_rect, &occupied, scale, tray_reported);
    log_plan(taskbar_rect, &occupied, dpi, bar);

    let Some(bar) = bar else {
        log_no_space();
        return None;
    };
    if !bar_is_clear(bar, &occupied) {
        return None;
    }

    occupied.shrink_to_fit();
    Some(Plan {
        taskbar,
        taskbar_rect,
        bar,
        occupied,
        tray_reported,
    })
}

/// 托盘整块当硬边界：UIA 的控件列表会漏掉「显示隐藏的图标」这类展开键，
/// 但 TrayNotifyWnd 的外沿涵盖它，宁可少占一点也不能压住。
///
/// 返回值第二项表示「托盘区里的控件已经被 UIA 读到」——读到了就不必再留
/// 大边距躲未知控件，按真实边界贴过去即可。
fn occupied_with_tray(
    taskbar: HWND,
    taskbar_rect: Rect,
    scale: f32,
    mut occupied: Vec<Rect>,
) -> (Vec<Rect>, bool) {
    let tray = unsafe { FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), PCWSTR::null()) }
        .ok()
        .and_then(window_rect);
    let Some(tray) = tray else {
        return (occupied, false);
    };
    // 展开键 / 托盘图标都落在托盘外沿左侧的这段范围内。
    let probe_zone = (CHEVRON_ZONE * scale) as i32;
    let reported = occupied
        .iter()
        .any(|rect| rect.left >= tray.left - probe_zone);
    occupied.push(Rect {
        left: tray.left,
        top: taskbar_rect.top,
        right: taskbar_rect.right,
        bottom: taskbar_rect.bottom,
    });
    log_event(
        LogLevel::Debug,
        LogTopic::App,
        "taskbar_probe_tray",
        &[
            ("rect", &format_rect(tray)),
            ("reported", if reported { "true" } else { "false" }),
        ],
    );
    (occupied, reported)
}

/// 按当前占用算播放条矩形；`None` 表示没有够宽的位置。
///
/// `reported` 为真时用收窄的边距（真实边界已知），否则按保守边距躲整块托盘。
fn place_bar(taskbar_rect: Rect, occupied: &[Rect], scale: f32, reported: bool) -> Option<Rect> {
    let margin = if reported {
        TRAY_MARGIN_CLOSE
    } else {
        TRAY_MARGIN
    };
    let gaps = geometry::free_gaps(taskbar_rect, occupied);
    geometry::place(
        &gaps,
        (BAR_WIDTH * scale) as i32,
        (BAR_MIN_WIDTH * scale) as i32,
        (BAR_INSET * scale) as i32,
        (margin * scale) as i32,
    )
}

fn bar_is_clear(bar: Rect, occupied: &[Rect]) -> bool {
    for rect in occupied {
        if let Some(hit) = bar.intersect(*rect) {
            log_event(
                LogLevel::Error,
                LogTopic::App,
                "taskbar_probe_overlap",
                &[("hit", &format_rect(hit))],
            );
            return false;
        }
    }
    true
}

fn log_plan(taskbar: Rect, occupied: &[Rect], dpi: u32, bar: Option<Rect>) {
    let gaps = geometry::free_gaps(taskbar, occupied);
    log_event(
        LogLevel::Info,
        LogTopic::App,
        "taskbar_probe_plan",
        &[
            ("taskbar", &format_rect(taskbar)),
            ("occupied", &format_rects(occupied)),
            ("gaps", &format_rects(&gaps)),
            ("dpi", &dpi.to_string()),
            ("bar", &bar.map(format_rect).unwrap_or_default()),
        ],
    );
}

fn log_probe_uncertain() {
    log_event(
        LogLevel::Warn,
        LogTopic::App,
        "taskbar_probe_uncertain",
        &[("detail", "没有任何可识别的占用控件")],
    );
}

fn log_no_space() {
    log_event(LogLevel::Info, LogTopic::App, "taskbar_probe_no_space", &[]);
}

fn log_probe_failed(detail: &str) {
    log_event(
        LogLevel::Warn,
        LogTopic::App,
        "taskbar_uia_failed",
        &[("detail", detail)],
    );
}

fn attach(hwnd_ptr: *mut c_void, plan: Plan) {
    let hwnd = HWND(hwnd_ptr);
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED.store(taskbar_created, Ordering::Relaxed);

    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let ex = WINDOW_EX_STYLE((ex | WS_EX_NOACTIVATE.0 as isize) as u32);
        SetLastError(ERROR_SUCCESS);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex.0 as isize);
        let style_error = GetLastError();
        if style_error != ERROR_SUCCESS {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_style_failed",
                &[("error", &style_error.0.to_string())],
            );
        }
        // 把任务栏设为所有者：owned 窗口始终排在所有者上方，
        // 点任务栏/托盘时不会先被任务栏盖住、再被抬回来（那一下就是闪烁）。
        SetLastError(ERROR_SUCCESS);
        let previous_owner = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, plan.taskbar.0 as isize);
        let owner_error = GetLastError();
        if previous_owner == 0 && owner_error != ERROR_SUCCESS {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_owner_failed",
                &[("error", &owner_error.0.to_string())],
            );
        }

        if let Err(error) = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            plan.bar.left,
            plan.bar.top,
            plan.bar.width(),
            plan.bar.height(),
            SWP_NOACTIVATE,
        ) {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_position_failed",
                &[("detail", &error.to_string())],
            );
        }
        if !SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0).as_bool() {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_subclass_failed",
                &[],
            );
        }
    }

    let hooks = unsafe { install_hooks(plan.taskbar) };

    log_event(
        LogLevel::Info,
        LogTopic::App,
        "taskbar_probe_attached",
        &[
            ("hwnd", &format!("{:?}", hwnd.0)),
            ("bar", &format_rect(plan.bar)),
            ("taskbar", &format_rect(plan.taskbar_rect)),
            ("hooks", &hooks.len().to_string()),
        ],
    );

    HOST.with(|slot| {
        *slot.borrow_mut() = Some(Host {
            hwnd,
            taskbar: plan.taskbar,
            epoch: 0,
            restarts: EXPLORER_RESTARTS.load(Ordering::Relaxed),
            taskbar_rect: plan.taskbar_rect,
            bar: plan.bar,
            state: TaskbarHostState::Attached,
            occupied: plan.occupied,
            tray_reported: plan.tray_reported,
            generation: 0,
            has_space: true,
            probe_pending: false,
            view_shown: false,
            dirty_since: None,
            last_probe: std::time::Instant::now(),
            hooks,
            last_foreground_events: 0,
            last_taskbar_events: 0,
            last_raise: std::time::Instant::now(),
            window: None,
            hide_at: None,
            misplaced: None,
        });
    });
}

/// 同步任务栏状态：收探测结果、触发重探、计算显隐和位置。
/// 返回 `false` 表示宿主已经不存在，循环可以退出。
fn sync(cx: &mut App, epoch: u64) -> bool {
    let foreground_events = FOREGROUND_EVENTS.load(Ordering::Relaxed);
    let taskbar_events = TASKBAR_EVENTS.load(Ordering::Relaxed);

    HOST.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(host) = slot.as_mut() else {
            return false;
        };
        // 关闭后立刻重开：旧循环不能继续操作新实例。
        if host.epoch != epoch {
            return false;
        }

        // 1) 收后台探测结果：只采用与当前代次相符的那份，旧结果直接丢。
        if let Some(outcome) = probe::take_result() {
            if probe_is_current(outcome.generation(), host.generation) {
                host.probe_pending = false;
                apply_probe(host, outcome);
            } else {
                // 旧代次结果：不能清 `probe_pending`，当前代次的那份还在路上。
                log_event(
                    LogLevel::Debug,
                    LogTopic::App,
                    "taskbar_probe_stale",
                    &[
                        ("generation", &outcome.generation().to_string()),
                        ("current", &host.generation.to_string()),
                    ],
                );
            }
        }

        // 2) 布局事件 → 标脏（150ms 合并窗口），5s 兜底重探。
        if host.probe_pending && host.last_probe.elapsed() >= PROBE_TIMEOUT {
            // 后台线程 panic / 卡死：结果不会回来了，解锁并重新排一次探测。
            host.probe_pending = false;
            host.dirty_since = Some(std::time::Instant::now());
            log_event(LogLevel::Warn, LogTopic::App, "taskbar_probe_timeout", &[]);
        }
        let events_changed = foreground_events != host.last_foreground_events
            || taskbar_events != host.last_taskbar_events;
        if events_changed {
            host.last_foreground_events = foreground_events;
            host.last_taskbar_events = taskbar_events;
        }
        let taskbar_rect = window_rect(host.taskbar);
        let rect_changed = taskbar_rect != Some(host.taskbar_rect);
        if events_changed || rect_changed || host.last_probe.elapsed() >= PROBE_FALLBACK {
            let now = std::time::Instant::now();
            host.dirty_since.get_or_insert(now);
        }
        let due = host
            .dirty_since
            .is_some_and(|since| since.elapsed() >= PROBE_DEBOUNCE);
        if due && !host.probe_pending {
            if let Some(generation) = probe::request(host.taskbar.0 as usize) {
                host.generation = generation;
                host.probe_pending = true;
            }
            host.dirty_since = None;
            host.last_probe = std::time::Instant::now();
        }

        // 3) 任务栏挪位置时先用现有占用快速重排，正式结果随后由探测覆盖。
        if let Some(rect) = taskbar_rect.filter(|rect| *rect != host.taskbar_rect) {
            host.taskbar_rect = rect;
            if let Some(next) = place_bar(
                rect,
                &host.occupied,
                scale_of(host.taskbar),
                host.tray_reported,
            ) {
                if next != host.bar {
                    host.bar = next;
                    unsafe { position(host.hwnd, next) };
                    log_event(
                        LogLevel::Info,
                        LogTopic::App,
                        "taskbar_probe_reposition",
                        &[("bar", &format_rect(next))],
                    );
                }
            }
        }

        // 4) 状态机：任务栏可见 → 全屏 → 有没有位置。
        let taskbar_visible = unsafe { IsWindowVisible(host.taskbar) }.as_bool();
        let fullscreen = covers_monitor(host.hwnd, host.taskbar);
        let next = state_for(
            taskbar_visible && taskbar_rect.is_some(),
            fullscreen,
            host.has_space,
        );
        if next != host.state {
            log_event(
                LogLevel::Info,
                LogTopic::App,
                "taskbar_probe_state",
                &[
                    ("from", &format!("{:?}", host.state)),
                    ("to", &format!("{:?}", next)),
                ],
            );
            host.state = next;
        }

        // 5) 显示/隐藏按状态走。安全相关的隐藏是立即的，没有动画。
        let should_show = next == TaskbarHostState::Attached;
        let now = std::time::Instant::now();
        let actual_visible = unsafe { IsWindowVisible(host.hwnd) }.as_bool();
        if !should_show {
            host.view_shown = false;
            // 全屏/自动隐藏这类系统状态给 250ms 淡出；空间冲突等安全状态立即隐藏。
            let fade_out = matches!(
                next,
                TaskbarHostState::HiddenFullscreen | TaskbarHostState::HiddenAutoHide
            );
            if actual_visible {
                set_view_visible(cx, host, false);
                if fade_out {
                    host.hide_at.get_or_insert(now + FADE_OUT);
                } else {
                    log_event(
                        LogLevel::Info,
                        LogTopic::App,
                        "taskbar_probe_hide",
                        &[("faded", "false")],
                    );
                    unsafe {
                        let _ = ShowWindow(host.hwnd, SW_HIDE);
                    }
                    host.hide_at = None;
                }
            }
            if host.hide_at.is_some_and(|at| now >= at) {
                log_event(
                    LogLevel::Info,
                    LogTopic::App,
                    "taskbar_probe_hide",
                    &[("faded", "true")],
                );
                unsafe {
                    let _ = ShowWindow(host.hwnd, SW_HIDE);
                }
                host.hide_at = None;
            }
            return true;
        }

        // 显示：首次定准、淡出被打断，或窗口被系统隐藏时，摆好并让视图淡入。
        if !host.view_shown || !actual_visible {
            host.hide_at = None;
            unsafe {
                position(host.hwnd, host.bar);
                if !actual_visible {
                    let _ = ShowWindow(host.hwnd, SW_SHOWNA);
                }
            }
            host.view_shown = true;
            set_view_visible(cx, host, true);
            log_event(LogLevel::Info, LogTopic::App, "taskbar_probe_show", &[]);
        }

        // 6) 开始菜单/搜索会重排 Z 序，把我们压到下面。只在「确认被压住 +
        // 没有 shell 浮层在前台」时抬一次，600ms 限流，不和菜单抢。
        if above_taskbar(host.hwnd, host.taskbar)
            && !shell_foreground()
            && host.last_raise.elapsed() >= std::time::Duration::from_millis(600)
        {
            host.last_raise = std::time::Instant::now();
            unsafe { raise(host.hwnd) };
            log_event(LogLevel::Debug, LogTopic::App, "taskbar_probe_raise", &[]);
        }

        // 7) 位置偏差只观测不纠正（gpui 会把窗口摆回自己的 bounds）。
        if let Some(actual) = window_rect(host.hwnd) {
            if actual != host.bar {
                if host.misplaced.as_ref() != Some(&actual) {
                    log_event(
                        LogLevel::Warn,
                        LogTopic::App,
                        "taskbar_probe_misplaced",
                        &[
                            ("actual", &format_rect(actual)),
                            ("target", &format_rect(host.bar)),
                        ],
                    );
                    host.misplaced = Some(actual);
                }
            } else {
                host.misplaced = None;
            }
        }
        true
    })
}

/// 把一次探测结论落到宿主状态上。任何不达标的结论都按「没空间」处理。
fn apply_probe(host: &mut Host, outcome: probe::Outcome) {
    match outcome {
        probe::Outcome::Valid { occupied, .. } => {
            let scale = scale_of(host.taskbar);
            let (occupied, reported) =
                occupied_with_tray(host.taskbar, host.taskbar_rect, scale, occupied);
            host.occupied = occupied;
            host.tray_reported = reported;
            match place_bar(host.taskbar_rect, &host.occupied, scale, reported) {
                Some(bar) => {
                    let moved = bar != host.bar;
                    host.bar = bar;
                    host.has_space = true;
                    if moved {
                        unsafe { position(host.hwnd, bar) };
                    }
                    log_event(
                        LogLevel::Info,
                        LogTopic::App,
                        "taskbar_probe_apply",
                        &[
                            ("bar", &format_rect(bar)),
                            ("occupied", &format_rects(&host.occupied)),
                            ("moved", if moved { "true" } else { "false" }),
                        ],
                    );
                }
                None => {
                    host.has_space = false;
                    log_event(
                        LogLevel::Warn,
                        LogTopic::App,
                        "taskbar_probe_no_space",
                        &[("occupied", &format_rects(&host.occupied))],
                    );
                }
            }
        }
        probe::Outcome::Uncertain { .. } => {
            host.has_space = false;
            log_probe_uncertain();
        }
        probe::Outcome::Failed { detail, .. } => {
            host.has_space = false;
            log_probe_failed(&detail);
        }
    }
}

/// 把显隐状态推给播放条视图（视图自己负责淡入淡出）。
fn set_view_visible(cx: &mut App, host: &Host, visible: bool) {
    let Some(window) = host.window.as_ref() else {
        return;
    };
    if let Err(error) = window.update(cx, |view, _window, cx| {
        if view.set_visible(visible) {
            cx.notify();
        }
    }) {
        log_event(
            LogLevel::Warn,
            LogTopic::App,
            "taskbar_view_update_failed",
            &[("detail", &format!("{error:?}"))],
        );
    }
}

fn scale_of(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// 任务栏矩形变化后按同样的留白规则重算，宽度不足就沿用旧宽度。
unsafe fn position(hwnd: HWND, rect: Rect) {
    unsafe {
        if let Err(error) = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            SWP_NOACTIVATE,
        ) {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_position_failed",
                &[("detail", &error.to_string())],
            );
        }
    }
}

/// 只把窗口抬回任务栏上面，不动位置/尺寸、不重新显示。
/// 抬层级用完整的 `SetWindowPos` 会让任务栏条闪一帧。
unsafe fn raise(hwnd: HWND) {
    unsafe {
        if let Err(error) = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        ) {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_raise_failed",
                &[("detail", &error.to_string())],
            );
        }
    }
}

/// 前台窗口是否盖满它所在的显示器（排除自己与任务栏/桌面）。
fn covers_monitor(ours: HWND, taskbar: HWND) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() || foreground == ours || foreground == taskbar {
            return false;
        }
        // 最小化或正在关闭的窗口仍可能短暂是前台，它们不可能盖住显示器；
        // 不加这一步，关闭/最小化主窗口的瞬间会把播放条误判成全屏而闪一下。
        if !IsWindowVisible(foreground).as_bool() || IsIconic(foreground).as_bool() {
            return false;
        }
        let class = window_class(foreground);
        // 桌面与 shell 背景窗口不算全屏。
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false;
        }
        // 自己进程的窗口（主播放器最大化等）不算全屏，否则会把播放条自己藏掉。
        let mut pid = 0u32;
        GetWindowThreadProcessId(foreground, Some(&mut pid as *mut u32));
        if pid == std::process::id() {
            return false;
        }
        let Some(rect) = extended_frame_rect(foreground) else {
            return false;
        };
        let target = if !ours.is_invalid() { ours } else { taskbar };
        let our_monitor = MonitorFromWindow(target, MONITOR_DEFAULTTONEAREST);
        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST);
        if our_monitor != monitor {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }
        let screen = Rect::from_rect(info.rcMonitor);
        // 必须真正盖住显示器（留 2px 给不可见边框）：最大化窗口不盖任务栏，
        // 用 95% 这种宽松阈值会把任何最大化窗口都误判成全屏。
        rect.left <= screen.left + 2
            && rect.top <= screen.top + 2
            && rect.right >= screen.right - 2
            && rect.bottom >= screen.bottom - 2
    }
}

unsafe fn extended_frame_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if ok.is_ok() {
        return Some(Rect::from_rect(rect));
    }
    window_rect(hwnd)
}

fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    let rect = Rect::from_rect(rect);
    (!rect.is_empty()).then_some(rect)
}

fn window_class(hwnd: HWND) -> String {
    let mut buffer = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

/// 任务栏是否压在我们上面：沿 Z 序往上走，先碰到任务栏就说明被压住。
fn above_taskbar(hwnd: HWND, taskbar: HWND) -> bool {
    let mut current = hwnd;
    for _ in 0..64 {
        let prev = unsafe { GetWindow(current, GW_HWNDPREV) }.unwrap_or_default();
        if prev.is_invalid() {
            return false;
        }
        if prev == taskbar {
            return true;
        }
        current = prev;
    }
    false
}

/// 桌面 / 任务栏 / 开始菜单在前台时不要抢 Z 序：此时任务栏本来就在最前，
/// 再抬播放条只会让它闪一下。
fn shell_foreground() -> bool {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_invalid() {
        return false;
    }
    let class = window_class(foreground);
    matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd")
        || class.contains("CoreWindow")
        || class.contains("XamlExplorerHost")
        || class.contains("TopLevelWindow")
}

/// 探针用的两个钩子：前台切换（全桌面）与任务栏事件（只限任务栏所在进程）。
unsafe fn install_hooks(taskbar: HWND) -> Vec<HWINEVENTHOOK> {
    let mut hooks = Vec::new();
    unsafe {
        let foreground = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_hook),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if !foreground.is_invalid() {
            hooks.push(foreground);
        } else {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_hook_failed",
                &[("hook", "foreground")],
            );
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(taskbar, Some(&mut pid as *mut u32));
        if pid != 0 {
            let taskbar_hook = SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(taskbar_hook),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if !taskbar_hook.is_invalid() {
                hooks.push(taskbar_hook);
            } else {
                log_event(
                    LogLevel::Warn,
                    LogTopic::App,
                    "taskbar_hook_failed",
                    &[("hook", "taskbar")],
                );
            }
        } else {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_hook_failed",
                &[("hook", "taskbar_pid_zero")],
            );
        }
    }
    hooks
}

unsafe extern "system" fn foreground_hook(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    FOREGROUND_EVENTS.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "system" fn taskbar_hook(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    TASKBAR_EVENTS.fetch_add(1, Ordering::Relaxed);
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == windows::Win32::UI::WindowsAndMessaging::WM_NCDESTROY {
        unsafe {
            let _ =
                windows::Win32::UI::Shell::RemoveWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID);
            return DefSubclassProc(hwnd, msg, wparam, lparam);
        }
    }
    // 点击不激活窗口：鼠标消息照常送到控件，但不抢前台。
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if msg == WM_DISPLAYCHANGE || msg == WM_SETTINGCHANGE {
        // 分辨率 / DPI / 任务栏设置变化：让 200ms 的状态同步下一拍重探，
        // 不用等 5s 兜底。
        TASKBAR_EVENTS.fetch_add(1, Ordering::Relaxed);
        log_event(
            LogLevel::Debug,
            LogTopic::App,
            "taskbar_probe_system_change",
            &[("msg", &msg.to_string())],
        );
        return unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    }
    // 不能在这里借 HOST：`sync` 里的 SetWindowPos 会同步派发消息进来，
    // 那个 borrow_mut 还没释放，再 borrow 一次就是 RefCell panic（实测）。
    let created = TASKBAR_CREATED.load(Ordering::Relaxed);
    if created == msg && msg != 0 {
        EXPLORER_RESTARTS.fetch_add(1, Ordering::Relaxed);
        log_event(
            LogLevel::Info,
            LogTopic::App,
            "taskbar_probe_taskbar_created",
            &[],
        );
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn format_rect(rect: Rect) -> String {
    format!(
        "({}, {}, {}, {})",
        rect.left, rect.top, rect.right, rect.bottom
    )
}

fn format_rects(rects: &[Rect]) -> String {
    rects
        .iter()
        .map(|rect| format!("{}-{}", rect.left, rect.right))
        .collect::<Vec<_>>()
        .join(",")
}

fn native_hwnd(window: &gpui::Window) -> Option<*mut c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
        _ => None,
    }
}

/// 关闭开关：先摘钩子、隐藏并销毁窗口，再清掉宿主状态（tick 循环随即退出）。
pub(crate) fn stop(cx: &mut App) {
    HOST_EPOCH.fetch_add(1, Ordering::Relaxed);
    STARTING.store(false, Ordering::Relaxed);
    let host = HOST.with(|slot| slot.borrow_mut().take());
    let Some(mut host) = host else {
        return;
    };
    unsafe {
        for hook in host.hooks.drain(..) {
            if !UnhookWinEvent(hook).as_bool() {
                log_event(LogLevel::Warn, LogTopic::App, "taskbar_unhook_failed", &[]);
            }
        }
        let _ = ShowWindow(host.hwnd, SW_HIDE);
    }
    if let Some(window) = host.window.take() {
        if let Err(error) = window.update(cx, |_, window, _| window.remove_window()) {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_window_remove_failed",
                &[("detail", &format!("{error:?}"))],
            );
        }
    }
    log_event(LogLevel::Info, LogTopic::App, "taskbar_player_stopped", &[]);
}
