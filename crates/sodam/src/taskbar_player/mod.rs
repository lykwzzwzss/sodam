//! 任务栏快捷播控：贴在 Windows 任务栏空闲区的播放条。
//!
//! 宿主层负责「能不能放、放哪里」：UIA 占用区间 → 空闲区间 → 落位，
//! 探测带代次，旧结果一律丢弃；判定不可靠就隐藏，绝不靠猜。
//! 视图只读 `Root` 的播放状态，并把按钮点击转发回 `Root`。
//!
//! 开关是设置里的 `taskbar_player`（顶栏按钮即时起停）；
//! `SODAM_TASKBAR_PROBE` 设置任意值可在启动时强制开启，调试用。

#[cfg(target_os = "windows")]
mod geometry;
#[cfg(target_os = "windows")]
mod host;
#[cfg(target_os = "windows")]
mod probe;
#[cfg(target_os = "windows")]
mod view;

/// 物理像素矩形。宿主层与探测层共用。
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[cfg(target_os = "windows")]
impl Rect {
    pub(crate) fn from_rect(rect: windows::Win32::Foundation::RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    pub(crate) fn width(self) -> i32 {
        self.right - self.left
    }

    pub(crate) fn height(self) -> i32 {
        self.bottom - self.top
    }

    pub(crate) fn is_empty(self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub(crate) fn intersect(self, other: Self) -> Option<Self> {
        let rect = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (!rect.is_empty()).then_some(rect)
    }
}

/// 宿主状态。用户开关关闭时宿主直接销毁，不进入状态机。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskbarHostState {
    HiddenNoSpace,
    HiddenFullscreen,
    HiddenAutoHide,
    Attached,
}

/// 由三个布尔量决定宿主状态，顺序即优先级（任务栏都没了就不用看别的）。
#[cfg(target_os = "windows")]
pub(crate) fn state_for(
    taskbar_visible: bool,
    fullscreen: bool,
    has_space: bool,
) -> TaskbarHostState {
    if !taskbar_visible {
        TaskbarHostState::HiddenAutoHide
    } else if fullscreen {
        TaskbarHostState::HiddenFullscreen
    } else if !has_space {
        TaskbarHostState::HiddenNoSpace
    } else {
        TaskbarHostState::Attached
    }
}

/// 探测结果是否属于当前代次：晚到的旧结果一律丢弃。
#[cfg(target_os = "windows")]
pub(crate) fn probe_is_current(generation: u64, latest: u64) -> bool {
    generation == latest
}

/// 启动时按配置决定是否挂上宿主；`SODAM_TASKBAR_PROBE=1` 强制开启（调试用）。
pub(crate) fn start_if_enabled(
    cx: &mut gpui::App,
    root: gpui::Entity<crate::app::Root>,
    enabled: bool,
) {
    #[cfg(target_os = "windows")]
    {
        if enabled || std::env::var("SODAM_TASKBAR_PROBE").is_ok() {
            host::start(cx, root);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (cx, root, enabled);
    }
}

/// 用户开关：立刻起停宿主，不等重启。
pub(crate) fn set_enabled(cx: &mut gpui::App, root: gpui::Entity<crate::app::Root>, enabled: bool) {
    #[cfg(target_os = "windows")]
    {
        if enabled {
            host::start(cx, root);
        } else {
            host::stop(cx);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (cx, root, enabled);
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{probe_is_current, state_for, TaskbarHostState};

    #[test]
    fn state_priority_is_taskbar_then_fullscreen_then_space() {
        assert_eq!(
            state_for(false, true, false),
            TaskbarHostState::HiddenAutoHide
        );
        assert_eq!(
            state_for(true, true, false),
            TaskbarHostState::HiddenFullscreen
        );
        assert_eq!(
            state_for(true, false, false),
            TaskbarHostState::HiddenNoSpace
        );
        assert_eq!(state_for(true, false, true), TaskbarHostState::Attached);
    }

    #[test]
    fn stale_probe_generations_are_rejected() {
        assert!(probe_is_current(5, 5));
        assert!(!probe_is_current(4, 5));
        assert!(!probe_is_current(6, 5));
    }
}
