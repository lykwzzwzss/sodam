//! Windows 自绘标题栏：拖拽区 + 最小化/最大化/关闭按钮。
//!
//! gpui Windows 后端在 `TitlebarOptions::appears_transparent` 时会移除系统
//! 标题栏（WM_NCCALCSIZE），由应用通过 `window_control_area` 提供控制区：
//! WM_NCHITTEST 把这些区域映射为 HTCAPTION / HTMINBUTTON / HTMAXBUTTON /
//! HTCLOSE，拖拽、双击最大化、Win11 贴靠布局全部由系统处理。
//!
//! 命中匹配取「第一个包含光标的 hitbox」，因此按钮必须与拖拽区是
//! 兄弟节点，不能嵌套 —— 否则拖拽区先匹配，按钮会失效。
//! Linux（Wayland 会话）下 gpui 只能提供客户端装饰，系统不会画标题栏；而
//! `on_hit_test_window_control` 在 Linux 后端是空实现，`window_control_area`
//! 不生效。因此 Linux 下按钮点击与标题栏拖拽由本模块直接调用窗口 API 处理。
//!
//! 拖拽**不能**在按下时就 `start_window_move()`：合成器会立刻对指针做交互移动
//! 抓取，双击的第二下被吞掉，`click_count >= 2` 永远收不到（实测）。所以按下只
//! 记录锚点，移动超过 [`DRAG_THRESHOLD`] 才交给合成器。
//!
//! macOS 走系统红绿灯，不使用本组件。

use gpui::prelude::*;
#[cfg(target_os = "linux")]
use gpui::MouseButton;
use gpui::{div, px, svg, AnimationExt, Context, Div, Pixels, Point, Window, WindowControlArea};
use std::{cell::Cell, rc::Rc};

use crate::app::Root;
use crate::ui::motion::{self, HoverFade};
use crate::ui::{icons, theme};

/// 单个窗口控制按钮宽度：汽水音乐 138 宽三等分，按整体 1.25 倍缩放。
const BUTTON_WIDTH: f32 = 55.0;
/// 控制图标画布边长；实际线条约 13~14px，占标题栏高度的近 1/4，与放大的
/// 文件夹动作图标保持视觉平衡（汽水音乐同比例）。
const GLYPH_SIZE: f32 = 18.0;
/// 拖拽阈值（逻辑像素）：移动超过它才把指针交给合成器移动窗口。
/// 太小会让手抖误触发、太大拖拽发涩，参考各桌面工具包的 3~4px。
#[cfg(target_os = "linux")]
const DRAG_THRESHOLD: f32 = 4.0;

/// 标题栏拖拽的按下锚点（由 `Root` 持有，跨帧保留）。
/// Windows/macOS 由系统处理拖拽，这里只是保持签名一致。
pub type DragAnchor = Rc<Cell<Option<Point<Pixels>>>>;

/// 顶栏右侧的三个窗口控制按钮（最小化/最大化/关闭）。
///
/// 新版顶栏把按钮直接排进工具条，而不是覆盖一层全宽标题栏：这样搜索/文件夹
/// 胶囊可以放在同一行，视觉上与汽水音乐一致。
pub(crate) fn controls(root: &Root, window: &Window, cx: &mut Context<Root>) -> Div {
    // 汽水音乐：三个等宽按钮直接占满顶栏高度，没有分隔线，
    // 悬停时整格变色（关闭格是品牌红），没有圆角。
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h_full()
        // 应用动作与窗口控制之间的竖向分隔线：左侧是内容操作，右侧是系统窗口。
        .child(control_divider())
        .child(caption_button(
            "titlebar-min",
            "windows-minimize",
            WindowControlArea::Min,
            false,
            root.hovers.get("titlebar-min"),
            cx,
        ))
        .child(caption_button(
            "titlebar-max",
            maximize_icon(window.is_maximized()),
            WindowControlArea::Max,
            false,
            root.hovers.get("titlebar-max"),
            cx,
        ))
        .child(caption_button(
            "titlebar-close",
            "windows-close",
            WindowControlArea::Close,
            true,
            root.hovers.get("titlebar-close"),
            cx,
        ))
}

/// 最大化后按钮换成「还原」图标，和系统窗口的语义一致。
fn maximize_icon(is_maximized: bool) -> &'static str {
    if is_maximized {
        "windows-restore"
    } else {
        "windows-maximize"
    }
}

/// 应用动作与窗口控制之间的 1px 分隔线。
fn control_divider() -> impl IntoElement {
    div()
        .flex_none()
        .w(px(1.0))
        .h(px(24.0))
        // 左边是应用动作，右边第一个是 55px 宽的窗口按钮（图标居中）；
        // 左侧留多一点，分隔线看起来才落在两组图标的中间。
        .ml(px(theme::space::LG))
        .mr(px(theme::space::SM))
        .bg(theme::border_strong())
}

pub(crate) fn drag_area(anchor: DragAnchor) -> impl IntoElement {
    #[cfg(not(target_os = "linux"))]
    let _ = &anchor;
    let area = div()
        .id("titlebar-drag")
        .flex_1()
        .h_full()
        .window_control_area(WindowControlArea::Drag);

    // Linux 后端不处理控制区：双击标题栏等同于最大化/还原（Windows 由
    // WM_NCHITTEST 的 HTCAPTION 双击处理），拖拽则等移动超过阈值后再交给合成器。
    #[cfg(target_os = "linux")]
    let area = area
        .on_mouse_down(MouseButton::Left, {
            let anchor = anchor.clone();
            move |event, window, _| {
                if event.click_count >= 2 {
                    anchor.set(None);
                    window.zoom_window();
                } else {
                    anchor.set(Some(event.position));
                }
            }
        })
        .on_mouse_move({
            let anchor = anchor.clone();
            move |event, window, _| {
                let Some(origin) = anchor.get() else { return };
                if event.pressed_button != Some(MouseButton::Left) {
                    anchor.set(None);
                    return;
                }
                if drag_threshold_exceeded(origin, event.position) {
                    // 先清锚点：交给合成器后拿不到 mouse_up，不能靠它复位。
                    anchor.set(None);
                    window.start_window_move();
                }
            }
        })
        .on_mouse_up(MouseButton::Left, {
            let anchor = anchor.clone();
            move |_, _, _| anchor.set(None)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, _| anchor.set(None));

    area
}

/// 鼠标是否已从按下点移动超过拖拽阈值。
#[cfg(target_os = "linux")]
fn drag_threshold_exceeded(origin: Point<Pixels>, current: Point<Pixels>) -> bool {
    let dx: f32 = (current.x - origin.x).abs().into();
    let dy: f32 = (current.y - origin.y).abs().into();
    dx > DRAG_THRESHOLD || dy > DRAG_THRESHOLD
}

fn caption_button(
    id: &'static str,
    icon: &'static str,
    area: WindowControlArea,
    danger: bool,
    fade: HoverFade,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let resting_text = theme::text();
    let hover_text = if danger {
        theme::hex(0xFFFFFF)
    } else {
        theme::control_emphasis_text()
    };
    let base = div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(BUTTON_WIDTH))
        .h_full()
        .window_control_area(area)
        .child(motion::hover_layer(
            id,
            fade,
            0.0,
            if danger {
                theme::hex(0xC42B1C)
            } else {
                theme::caption_hover()
            },
        ))
        .on_hover(Root::hover_listener(cx, id))
        .child(if fade.seq == 0 {
            svg()
                .path(icons::path(icon))
                .size(px(GLYPH_SIZE))
                .text_color(if fade.hovered {
                    hover_text
                } else {
                    resting_text
                })
                .into_any_element()
        } else {
            let (from, to) = if fade.hovered {
                (resting_text, hover_text)
            } else {
                (hover_text, resting_text)
            };
            svg()
                .path(icons::path(icon))
                .size(px(GLYPH_SIZE))
                .with_animation(
                    gpui::ElementId::Name(format!("{id}-glyph-{}", fade.seq).into()),
                    motion::standard(motion::duration::HOVER),
                    move |this, delta| this.text_color(motion::mix_rgba(from, to, delta)),
                )
                .into_any_element()
        });

    // Windows 由 WM_NCHITTEST 把控制区映射成 HTMINBUTTON 等消息；Linux 后端
    // 不处理控制区，这里直接调用窗口 API，避免两个平台各点一次。
    #[cfg(target_os = "linux")]
    let base = base.on_click(move |_, window, _| match area {
        WindowControlArea::Min => window.minimize_window(),
        WindowControlArea::Max => window.zoom_window(),
        WindowControlArea::Close => window.remove_window(),
        _ => {}
    });

    base
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 双击要能收到第二下：按下时不能立刻开始移动，只有超过阈值才移交合成器。
    #[cfg(target_os = "linux")]
    #[test]
    fn drag_starts_only_after_threshold() {
        let origin = Point::new(px(100.0), px(50.0));
        // 手抖 / 双击时的微小位移不算拖拽
        assert!(!drag_threshold_exceeded(
            origin,
            Point::new(px(101.0), px(51.0))
        ));
        assert!(!drag_threshold_exceeded(
            origin,
            Point::new(px(103.9), px(46.1))
        ));
        // 越过阈值才开始移动窗口
        assert!(drag_threshold_exceeded(
            origin,
            Point::new(px(104.1), px(50.0))
        ));
        assert!(drag_threshold_exceeded(
            origin,
            Point::new(px(100.0), px(54.1))
        ));
    }

    #[test]
    fn maximize_button_switches_to_restore_icon() {
        assert_eq!(maximize_icon(false), "windows-maximize");
        assert_eq!(maximize_icon(true), "windows-restore");
    }
}
