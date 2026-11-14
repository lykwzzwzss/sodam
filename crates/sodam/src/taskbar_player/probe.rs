//! UIA 控件树探针。
//!
//! 采集任务栏的 UI Automation 树，挑出**实际占用**的控件矩形交给宿主算空闲区。
//! 三条规矩：
//! 1. 容器不算占用：`Pane / Group / Custom` 这类只是布局，盖住整条任务栏的大矩形直接丢；
//! 2. 结果带代次：晚到的旧探测不能覆盖新状态；
//! 3. 判不准就报 `Uncertain`，宿主宁可不显示，也不猜。
//!
//! 后台 MTA 线程执行，结果不接触 GPUI。

use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
    UIA_ButtonControlTypeId, UIA_CalendarControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_CustomControlTypeId, UIA_EditControlTypeId,
    UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId,
    UIA_ListItemControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SliderControlTypeId, UIA_TabItemControlTypeId,
    UIA_TextControlTypeId, UIA_ThumbControlTypeId, UIA_ToolBarControlTypeId,
    UIA_ToolTipControlTypeId, UIA_TreeControlTypeId, UIA_TreeItemControlTypeId,
    UIA_WindowControlTypeId,
};

use super::Rect;
use sodam_core::log_outlet::{enabled, log_dir, log_event, LogLevel, LogTopic};

/// 只枚举到第 6 层、最多 400 个节点：任务栏的树很浅，再深就是失控。
const MAX_DEPTH: usize = 6;
const MAX_NODES: usize = 400;

/// 一次探测的结论。`Uncertain` / `Failed` 都不允许拿来放播放条。
pub(crate) enum Outcome {
    Valid {
        generation: u64,
        occupied: Vec<Rect>,
    },
    Uncertain {
        generation: u64,
    },
    Failed {
        generation: u64,
        detail: String,
    },
}

impl Outcome {
    pub(crate) fn generation(&self) -> u64 {
        match self {
            Self::Valid { generation, .. }
            | Self::Uncertain { generation }
            | Self::Failed { generation, .. } => *generation,
        }
    }
}

/// 全局代次：每次请求 +1，晚到的旧结果按代次丢弃。
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// 后台探测的结果信箱（同一时间只允许一个待处理结果）。
static INBOX: Mutex<Option<Outcome>> = Mutex::new(None);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

/// 在探测线程内读 UIA。
fn scan_now(taskbar: usize, generation: u64) -> Outcome {
    let hwnd = HWND(taskbar as *mut c_void);
    let nodes = match unsafe { collect(hwnd) } {
        Ok(nodes) => nodes,
        Err(detail) => return Outcome::Failed { generation, detail },
    };
    report(&nodes);

    let width = taskbar_width_hint(&nodes);
    let (occupied, uncertain) = occupied_from(&nodes, width);
    if uncertain {
        return Outcome::Uncertain { generation };
    }
    Outcome::Valid {
        generation,
        occupied,
    }
}

/// 后台探测：结果放进信箱，由宿主 tick 取走。
///
/// 线程创建失败时返回 `None`，宿主不能把 `probe_pending` 置真，
/// 否则后续探测永远不会再发起。
pub(crate) fn request(taskbar: usize) -> Option<u64> {
    let generation = next_generation();
    let spawned = std::thread::Builder::new()
        .name("sodam-taskbar-probe".to_string())
        .spawn(move || {
            let outcome = scan_now(taskbar, generation);
            let mut slot = INBOX.lock().unwrap_or_else(|error| error.into_inner());
            // 超时后宿主会再发一次探测；旧线程晚到的结果不能盖掉新代次。
            let fresh = slot
                .as_ref()
                .is_none_or(|current| generation >= current.generation());
            if fresh {
                *slot = Some(outcome);
            }
        });
    match spawned {
        Ok(_) => Some(generation),
        Err(error) => {
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "taskbar_probe_thread_failed",
                &[("detail", &error.to_string())],
            );
            None
        }
    }
}

pub(crate) fn take_result() -> Option<Outcome> {
    INBOX
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}

/// 该控制类型是否真的占地方。容器类型（Pane/Group/Custom/Image…）一律不算。
fn occupies(control_type: &str) -> bool {
    matches!(
        control_type,
        "Button" | "MenuItem" | "Edit" | "TabItem" | "ListItem" | "ComboBox" | "Slider"
    )
}

/// 从树里挑占用矩形。返回 `(occupied, uncertain)`。
///
/// uncertain 的条件：树里一个占位控件都没有（说明这版任务栏结构读不懂），
/// 或者树被截断（可能漏控件）。
fn occupied_from(nodes: &[Node], taskbar_width: i32) -> (Vec<Rect>, bool) {
    let mut rects: Vec<Rect> = nodes
        .iter()
        .filter(|node| !node.offscreen && occupies(&node.control_type))
        .map(|node| Rect::from_rect(node.rect))
        .filter(|rect| !rect.is_empty())
        // 盖住整条任务栏的大矩形是容器（TaskbarFrame 这类），不是控件。
        .filter(|rect| taskbar_width <= 0 || rect.width() * 100 < taskbar_width * 95)
        .collect();
    rects.sort_by_key(|rect| rect.left);
    rects.dedup();
    let uncertain = rects.is_empty() || nodes.len() >= MAX_NODES;
    (rects, uncertain)
}

struct Node {
    depth: usize,
    control_type: String,
    name: String,
    class: String,
    rect: RECT,
    offscreen: bool,
    children: usize,
}

fn report(nodes: &[Node]) {
    let leaves = nodes.iter().filter(|node| node.children == 0).count();
    let wide = nodes
        .iter()
        .filter(|node| node.rect.right - node.rect.left >= taskbar_width_hint(nodes))
        .count();
    let offscreen = nodes.iter().filter(|node| node.offscreen).count();

    // 完整控件树只在调试日志打开时落盘；正常运行时每 5s 写一次太浪费。
    let dump = enabled(LogLevel::Debug, LogTopic::App);
    let mut text = String::new();
    if dump {
        for node in nodes {
            let _ = writeln!(
                text,
                "{:indent$}{} name={:?} class={:?} rect=({}, {}, {}, {}) offscreen={} children={}",
                "",
                node.control_type,
                node.name,
                node.class,
                node.rect.left,
                node.rect.top,
                node.rect.right,
                node.rect.bottom,
                node.offscreen,
                node.children,
                indent = node.depth * 2,
            );
        }
    }

    let path = log_dir().join("uia-taskbar.txt");
    let written = if dump {
        std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .and_then(|_| std::fs::write(&path, text.as_bytes()))
            .is_ok()
    } else {
        false
    };

    let leaves_field = leaves.to_string();
    let wide_field = wide.to_string();
    let offscreen_field = offscreen.to_string();
    let nodes_field = nodes.len().to_string();
    let path_field = path.display().to_string();
    log_event(
        LogLevel::Debug,
        LogTopic::App,
        "taskbar_uia_dump",
        &[
            ("nodes", nodes_field.as_str()),
            ("leaves", leaves_field.as_str()),
            ("wide", wide_field.as_str()),
            ("offscreen", offscreen_field.as_str()),
            ("written", if written { "true" } else { "false" }),
            ("path", path_field.as_str()),
        ],
    );
}

/// 用最宽的根矩形当作「任务栏宽度」的参照，仅用于统计广域容器。
fn taskbar_width_hint(nodes: &[Node]) -> i32 {
    nodes
        .iter()
        .map(|node| node.rect.right - node.rect.left)
        .max()
        .unwrap_or(0)
}

struct ComGuard(bool);

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

unsafe fn collect(hwnd: HWND) -> Result<Vec<Node>, String> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        // 只有本次真的初始化成功才配对反初始化；失败时调用会多减一次引用计数。
        let _guard = ComGuard(hr.is_ok());
        // 线程是新起的，正常不会已经初始化；失败也继续（UIA 可能仍可用），但要留痕。
        if hr.is_err() {
            log_event(
                LogLevel::Debug,
                LogTopic::App,
                "taskbar_com_init_failed",
                &[("hr", &format!("{hr:?}"))],
            );
        }
        collect_inner(hwnd)
    }
}

unsafe fn collect_inner(hwnd: HWND) -> Result<Vec<Node>, String> {
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| format!("CoCreateInstance(CUIAutomation): {error}"))?;
    let root = unsafe { automation.ElementFromHandle(hwnd) }
        .map_err(|error| format!("ElementFromHandle: {error}"))?;
    let walker = unsafe { automation.ControlViewWalker() }
        .map_err(|error| format!("ControlViewWalker: {error}"))?;

    let mut nodes = Vec::new();
    unsafe { walk(&walker, &root, 0, &mut nodes) };
    // 某些任务栏提供程序只在 Raw View 暴露子控件。仍用同样的占用规则，
    // Raw View 也没有可识别的控件时由宿主保持隐藏。
    if nodes.len() == 1 {
        let raw_walker = unsafe { automation.RawViewWalker() }
            .map_err(|error| format!("RawViewWalker: {error}"))?;
        nodes.clear();
        unsafe { walk(&raw_walker, &root, 0, &mut nodes) };
    }
    if nodes.is_empty() {
        return Err("UIA 树为空".to_string());
    }
    Ok(nodes)
}

unsafe fn walk(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    if depth > MAX_DEPTH || nodes.len() >= MAX_NODES {
        return 0;
    }
    let index = nodes.len();
    nodes.push(unsafe { read(element, depth) });

    let mut children = 0;
    if depth < MAX_DEPTH && nodes.len() < MAX_NODES {
        let mut child = unsafe { walker.GetFirstChildElement(element) }.ok();
        while let Some(element) = child {
            children += 1;
            unsafe { walk(walker, &element, depth + 1, nodes) };
            if nodes.len() >= MAX_NODES {
                break;
            }
            child = unsafe { walker.GetNextSiblingElement(&element) }.ok();
        }
    }
    nodes[index].children = children;
    children
}

unsafe fn read(element: &IUIAutomationElement, depth: usize) -> Node {
    let control_type = unsafe { element.CurrentControlType() }
        .map(|id| control_type_name(id.0).to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let name = unsafe { element.CurrentName() }
        .map(|name| name.to_string())
        .unwrap_or_default();
    let class = unsafe { element.CurrentClassName() }
        .map(|class| class.to_string())
        .unwrap_or_default();
    let rect = unsafe { element.CurrentBoundingRectangle() }.unwrap_or_default();
    let offscreen = unsafe { element.CurrentIsOffscreen() }
        .map(|value| value.as_bool())
        .unwrap_or(false);

    Node {
        depth,
        control_type,
        name,
        class,
        rect,
        offscreen,
        children: 0,
    }
}

#[allow(non_upper_case_globals)]
fn control_type_name(id: i32) -> &'static str {
    let id = windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID(id);
    match id {
        UIA_ButtonControlTypeId => "Button",
        UIA_CalendarControlTypeId => "Calendar",
        UIA_CheckBoxControlTypeId => "CheckBox",
        UIA_ComboBoxControlTypeId => "ComboBox",
        UIA_CustomControlTypeId => "Custom",
        UIA_EditControlTypeId => "Edit",
        UIA_GroupControlTypeId => "Group",
        UIA_HyperlinkControlTypeId => "Hyperlink",
        UIA_ImageControlTypeId => "Image",
        UIA_ListItemControlTypeId => "ListItem",
        UIA_MenuControlTypeId => "Menu",
        UIA_MenuItemControlTypeId => "MenuItem",
        UIA_PaneControlTypeId => "Pane",
        UIA_ProgressBarControlTypeId => "ProgressBar",
        UIA_RadioButtonControlTypeId => "RadioButton",
        UIA_ScrollBarControlTypeId => "ScrollBar",
        UIA_SliderControlTypeId => "Slider",
        UIA_TabItemControlTypeId => "TabItem",
        UIA_TextControlTypeId => "Text",
        UIA_ThumbControlTypeId => "Thumb",
        UIA_ToolBarControlTypeId => "ToolBar",
        UIA_ToolTipControlTypeId => "ToolTip",
        UIA_TreeControlTypeId => "Tree",
        UIA_TreeItemControlTypeId => "TreeItem",
        UIA_WindowControlTypeId => "Window",
        _ => "Other",
    }
}

#[cfg(test)]
mod tests {
    use super::{occupied_from, Node};
    use windows::Win32::Foundation::RECT;

    fn node(control_type: &str, left: i32, right: i32, offscreen: bool) -> Node {
        Node {
            depth: 1,
            control_type: control_type.to_string(),
            name: String::new(),
            class: String::new(),
            rect: RECT {
                left,
                top: 0,
                right,
                bottom: 60,
            },
            offscreen,
            children: 0,
        }
    }

    #[test]
    fn containers_and_wide_frames_are_not_occupancy() {
        let nodes = vec![
            // 容器：ControlType 是 Pane，不能当占用。
            node("Pane", 0, 2560, false),
            // 结构上看是 Button，但盖住整条任务栏，同样按容器丢掉。
            node("Button", 0, 2560, false),
            node("Button", 0, 83, false),
            node("Button", 83, 149, false),
        ];
        let (occupied, uncertain) = occupied_from(&nodes, 2560);
        assert!(!uncertain);
        assert_eq!(occupied.len(), 2);
        assert_eq!((occupied[0].left, occupied[1].left), (0, 83));
    }

    #[test]
    fn offscreen_and_duplicate_rects_are_dropped() {
        let nodes = vec![
            node("Button", 0, 83, true),
            node("Button", 100, 200, false),
            node("Button", 100, 200, false),
        ];
        let (occupied, _) = occupied_from(&nodes, 2560);
        assert_eq!(occupied.len(), 1);
        assert_eq!((occupied[0].left, occupied[0].right), (100, 200));
    }

    #[test]
    fn an_empty_result_is_uncertain() {
        let nodes = vec![node("Pane", 0, 2560, false), node("Custom", 0, 2560, false)];
        let (occupied, uncertain) = occupied_from(&nodes, 2560);
        assert!(occupied.is_empty());
        assert!(uncertain);
    }
}
