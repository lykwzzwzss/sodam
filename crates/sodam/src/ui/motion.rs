//! 动效系统：统一的时长、缓动与跨组件复用的过渡原语。
//!
//! 设计原则（见 `docs/MOTION.md`）：
//!
//! 1. **轻盈**：位移 4–12px，缩放 0.96–1.02，不做夸张弹跳；
//! 2. **连续**：旧内容不突然消失，新内容自然接替（交叉淡化/双层封面）；
//! 3. **灵敏**：点击、拖拽、播放状态立即响应，动画只负责视觉衔接。
//!
//! 所有组件从这里取时长和缓动，不在各自的文件里再写 magic number。
//! 动画推进仍然由 GPUI 的 `with_animation` 驱动（基于单调时钟、逐帧），
//! 需要手动插值的地方（悬停进度、正在播放高亮）也用同一组常量。

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{
    div, img, px, Animation, AnimationExt, AnyElement, ElementId, IntoElement, ObjectFit, Rgba,
};

/// 统一时长梯度。数值来自动效规范，不要在使用处另写毫秒数。
pub mod duration {
    use std::time::Duration;

    /// 悬停：底色、图标颜色。
    pub const HOVER: Duration = Duration::from_millis(160);
    /// 焦点：搜索框描边。
    pub const FOCUS: Duration = Duration::from_millis(180);
    /// 图标状态切换：播放/暂停、静音、播放模式。
    pub const ICON: Duration = Duration::from_millis(200);
    /// 列表里「正在播放」行的高亮过渡。
    pub const PLAYING: Duration = Duration::from_millis(200);
    /// 小型弹出层：右键菜单。
    pub const MENU_IN: Duration = Duration::from_millis(220);
    pub const MENU_OUT: Duration = Duration::from_millis(150);
    /// 内容更新：标题、文字交叉淡化。
    pub const CONTENT: Duration = Duration::from_millis(220);
    /// 搜索模式进入/退出：头部布局调整，但输入过滤仍然即时。
    pub const SEARCH: Duration = Duration::from_millis(350);
    /// 歌单切换：标题与歌数（和头图共用同一个切换起点）。
    pub const PLAYLIST_CONTENT: Duration = Duration::from_millis(320);
    /// 同一张封面从 96px 升级到 320px：原位轻过渡，不重播整段切换。
    pub const COVER_UPGRADE: Duration = Duration::from_millis(120);
    /// 侧栏行悬停离开（进入仍用 `HOVER`）。
    pub const SIDEBAR_HOVER_OUT: Duration = Duration::from_millis(180);
    /// 侧栏旧行背景淡出 / 新行背景淡入。
    pub const SIDEBAR_BG_OUT: Duration = Duration::from_millis(240);
    pub const SIDEBAR_BG_IN: Duration = Duration::from_millis(320);
    /// 侧栏旧行文字变暗 / 新行文字变亮。
    pub const SIDEBAR_TEXT_OUT: Duration = Duration::from_millis(220);
    pub const SIDEBAR_TEXT_IN: Duration = Duration::from_millis(280);
    /// 侧栏细条：相邻行滑动、远距离交叉淡化。
    pub const SIDEBAR_MARKER_NEAR: Duration = Duration::from_millis(380);
    pub const SIDEBAR_MARKER_FAR: Duration = Duration::from_millis(280);
    /// 本地页 ⇄ 正在播放页。
    pub const PAGE: Duration = Duration::from_millis(460);
    /// 队列抽屉：展开稍慢、收起略快。
    pub const DRAWER_OPEN: Duration = Duration::from_millis(420);
    pub const DRAWER_CLOSE: Duration = Duration::from_millis(320);
    /// 封面更换：大封面 420ms，播放栏小封面 340ms。
    pub const COVER: Duration = Duration::from_millis(420);
    pub const COVER_SMALL: Duration = Duration::from_millis(340);
    /// 缩略图从占位变成封面。
    pub const THUMB: Duration = Duration::from_millis(160);
    /// 歌词换行：当前行放大/变亮（跟拍感更利落）。
    pub const LYRIC: Duration = Duration::from_millis(320);
    /// 上一行歌词退场（比进入略快，视线已经移到新行）。
    pub const LYRIC_OUT: Duration = Duration::from_millis(290);
    /// 译文亮度跟随换行（字号不变，只变颜色）。
    pub const LYRIC_TRANSLATION: Duration = Duration::from_millis(360);
    /// 译文显示/隐藏：渐隐渐显 + 4px 位移，歌词位置保持稳定。
    pub const LYRIC_TRANSLATION_TOGGLE: Duration = Duration::from_millis(420);
    /// 整块歌词首次加载/切歌淡入。
    pub const LYRIC_LOAD: Duration = Duration::from_millis(450);
    /// 任务栏播放条第二行（歌词/歌手）换行的交叉淡化。
    pub const TASKBAR_LYRIC: Duration = Duration::from_millis(280);
    /// 状态行。
    pub const STATUS_IN: Duration = Duration::from_millis(160);
    pub const STATUS_OUT: Duration = Duration::from_millis(140);
    /// 轻提示。
    pub const TOAST_IN: Duration = Duration::from_millis(200);
    pub const TOAST_OUT: Duration = Duration::from_millis(150);
    /// 主题色：比内容运动更慢一点稳定下来。
    pub const THEME: Duration = Duration::from_millis(800);
    /// 空态/错误条。
    pub const STATE: Duration = Duration::from_millis(220);
    /// 队列删除：行退出与剩余行归位。
    pub const QUEUE_ROW_OUT: Duration = Duration::from_millis(140);
    pub const QUEUE_REFLOW: Duration = Duration::from_millis(180);
}

/// 标准缓动：起步快、收尾稳（绝大多数状态切换）。
pub fn standard(duration: Duration) -> Animation {
    Animation::new(duration).with_easing(gpui::ease_out_quint())
}

/// 容器缓动：三次减速，适合页面切换、抽屉、大封面等大面积位移/缩放，避免前冲过猛。
pub fn container(duration: Duration) -> Animation {
    Animation::new(duration).with_easing(smooth_decel)
}

/// 柔和缓动：两端都慢，用于选中高亮、主题色这类氛围变化。
pub fn soft(duration: Duration) -> Animation {
    Animation::new(duration).with_easing(ease_in_out_cubic)
}

/// 三次 ease-out，比 ease_out_quint 更柔和均匀。
pub fn smooth_decel(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// 三次 ease-in-out（比 gpui 自带的二次版本更圆滑）。
pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let x = -2.0 * t + 2.0;
        1.0 - x * x * x / 2.0
    }
}

/// 线性进度（0..1），用于需要自己算颜色的地方。
pub fn progress(started: Instant, duration: Duration) -> f32 {
    let total = duration.as_secs_f32().max(f32::EPSILON);
    (started.elapsed().as_secs_f32() / total).clamp(0.0, 1.0)
}

/// 两个颜色按 `t` 线性混合（含 alpha）。主题色过渡用 OKLab，这里只做小范围插值。
pub fn mix_rgba(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

/// 把颜色透明度按比例缩放（用于「正在播放」底色的渐变）。
pub fn with_alpha(mut color: Rgba, alpha: f32) -> Rgba {
    color.a = alpha;
    color
}

// ---------------------------------------------------------------------------
// 悬停状态
// ---------------------------------------------------------------------------

/// 一个控件的悬停淡入淡出状态。
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct HoverFade {
    pub hovered: bool,
    /// 每次进入/离开加一；0 表示还没发生过变化（首帧直接落位，不播动画）。
    pub seq: u64,
}

/// 全窗控件的悬停状态。键是 `Hash`（字符串、行下标元组都可以），
/// 不需要为每个控件单独加字段。
#[derive(Default)]
pub(crate) struct HoverStates {
    map: HashMap<u64, HoverFade>,
}

fn hash_key(key: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

impl HoverStates {
    pub(crate) fn get(&self, key: impl Hash) -> HoverFade {
        self.map.get(&hash_key(key)).copied().unwrap_or_default()
    }

    /// 记录进入/离开。状态真正变化时返回 `true`（调用方据此重绘）。
    pub(crate) fn set(&mut self, key: impl Hash, hovered: bool) -> bool {
        let entry = self.map.entry(hash_key(key)).or_default();
        if entry.hovered == hovered {
            return false;
        }
        entry.hovered = hovered;
        entry.seq = entry.seq.wrapping_add(1);
        true
    }

    /// 窗口关闭/重建时清空：旧窗口的 hover 出去事件不会再来了。
    pub(crate) fn clear(&mut self) {
        self.map.clear();
    }
}

/// 悬停底色层：绝对定位铺满父元素，进入 120ms 淡入、离开 120ms 淡出。
///
/// 调用方把这一层放在内容的**前面**（GPUI 按子元素顺序绘制），
/// 父元素需要 `relative()`。
pub(crate) fn hover_layer(id: &str, fade: HoverFade, radius: f32, color: Rgba) -> AnyElement {
    let layer = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .rounded(px(radius))
        .bg(color);
    if fade.seq == 0 {
        return layer
            .opacity(if fade.hovered { 1.0 } else { 0.0 })
            .into_any_element();
    }
    let (from, to) = if fade.hovered { (0.0, 1.0) } else { (1.0, 0.0) };
    layer
        .with_animation(
            ElementId::Name(format!("{id}-hover-{}", fade.seq).into()),
            standard(duration::HOVER),
            move |this, delta| this.opacity(from + (to - from) * delta),
        )
        .into_any_element()
}

/// 列表行的悬停状态：同一时刻最多一行「当前悬停」、一行「刚离开」。
///
/// 虚拟列表的行渲染闭包拿不到 `Root`，所以用一个整表共享的状态，
/// 每行只比较自己的下标，避免给成百上千行各存一份状态。
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct RowHover {
    pub current: Option<usize>,
    pub previous: Option<usize>,
    pub seq: u64,
    pub changed_at: Option<Instant>,
}

impl RowHover {
    /// 列表内容整体换掉时直接清空：不要把「旧下标」当成刚离开的行去播淡出。
    pub(crate) fn clear(&mut self) {
        self.current = None;
        self.previous = None;
        self.changed_at = None;
    }

    /// 指针进入/离开某一行。返回是否发生变化。
    pub(crate) fn set(&mut self, next: Option<usize>) -> bool {
        if self.current == next {
            return false;
        }
        match (self.current, next) {
            // 从一行直接移到另一行
            (Some(_), Some(_)) => self.previous = self.current,
            // 离开列表：记住刚离开的行，淡出层才有对象
            (Some(_), None) => self.previous = self.current,
            // 进入一行：保留可能刚离开的那一行，通常两者发生在同一帧
            (None, Some(_)) => {}
            (None, None) => {}
        }
        self.current = next;
        self.seq = self.seq.wrapping_add(1);
        self.changed_at = Some(Instant::now());
        true
    }

    /// 这一行需要的悬停层：`(是否悬停, 代次)`。
    pub(crate) fn fade_for(&self, index: usize) -> Option<(bool, u64)> {
        if self.current == Some(index) {
            Some((true, self.seq))
        } else if self.previous == Some(index)
            && self
                .changed_at
                .is_some_and(|at| at.elapsed() < duration::HOVER * 2)
        {
            Some((false, self.seq))
        } else {
            None
        }
    }
}

/// 列表行悬停底色层；`fade` 来自 [`RowHover::fade_for`]。
pub(crate) fn row_hover_layer(
    id: &str,
    fade: Option<(bool, u64)>,
    radius: f32,
    color: Rgba,
) -> Option<AnyElement> {
    let (hovered, seq) = fade?;
    Some(hover_layer(id, HoverFade { hovered, seq }, radius, color))
}

// ---------------------------------------------------------------------------
// 封面双层交叉淡化
// ---------------------------------------------------------------------------

/// 封面显示状态：记录当前显示的图和上一张图，换图时做双层交叉淡化。
///
/// 旧图保持不动、新图在上层淡入；这样不会出现「先变暗再变亮」的空档，
/// 新图还没准备好时也继续显示旧图。
#[derive(Default)]
pub(crate) struct CoverFade {
    shown: Option<PathBuf>,
    previous: Option<PathBuf>,
    seq: u64,
}

impl CoverFade {
    /// 更新目标封面；返回是否真的发生了变化。
    pub(crate) fn update(&mut self, next: Option<PathBuf>) -> bool {
        if self.shown == next {
            return false;
        }
        self.previous = self.shown.take();
        self.shown = next;
        self.seq = self.seq.wrapping_add(1);
        true
    }
}

/// 生成封面层的元素（从下到上），调用方放进一个 `relative()` 的容器里。
///
/// - 有新封面：旧封面（或占位）在下，新封面 280ms 淡入 + 0.985→1 轻微放大；
/// - 新封面为空：占位在下，旧封面在上 1→0 淡出；
/// - 首帧：占位 + 新封面淡入。
pub(crate) fn cover_layers(
    id: &str,
    fade: &CoverFade,
    duration: Duration,
    radius: f32,
    placeholder: AnyElement,
) -> Vec<AnyElement> {
    cover_layers_scaled(id, fade, duration, radius, placeholder, 0.985)
}

/// 同 [`cover_layers`]，但可以指定新封面的起始缩放。
///
/// 歌单头图不需要「放大进入」：切换时只做透明度交叉，`from_scale = 1.0`。
pub(crate) fn cover_layers_scaled(
    id: &str,
    fade: &CoverFade,
    duration: Duration,
    radius: f32,
    placeholder: AnyElement,
    from_scale: f32,
) -> Vec<AnyElement> {
    let mut layers = Vec::with_capacity(2);
    let base = fade.shown.as_ref().and(fade.previous.as_ref());
    match base {
        Some(path) => layers.push(static_cover(path.clone(), radius)),
        None => layers.push(placeholder),
    }
    if let Some(path) = fade.shown.clone() {
        layers.push(animated_cover(
            path,
            format!("{id}-cover-in-{}", fade.seq),
            duration,
            (0.0, 1.0),
            (from_scale, 1.0),
            radius,
        ));
    } else if let Some(path) = fade.previous.clone() {
        layers.push(animated_cover(
            path,
            format!("{id}-cover-out-{}", fade.seq),
            duration,
            (1.0, 0.0),
            (1.0, 0.99),
            radius,
        ));
    }
    layers
}

fn static_cover(path: PathBuf, radius: f32) -> AnyElement {
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .rounded(px(radius))
        .overflow_hidden()
        .child(img(path).size_full().object_fit(ObjectFit::Cover))
        .into_any_element()
}

fn animated_cover(
    path: PathBuf,
    id: String,
    duration: Duration,
    opacity: (f32, f32),
    scale: (f32, f32),
    radius: f32,
) -> AnyElement {
    let (opacity_from, opacity_to) = opacity;
    let (scale_from, scale_to) = scale;
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .rounded(px(radius))
        .overflow_hidden()
        .with_animation(
            ElementId::Name(id.into()),
            standard(duration),
            move |this, delta| {
                this.opacity(opacity_from + (opacity_to - opacity_from) * delta)
                    .layer_scale(scale_from + (scale_to - scale_from) * delta)
            },
        )
        .child(img(path).size_full().object_fit(ObjectFit::Cover))
        .into_any_element()
}

/// 缩略图淡入：图片第一次出现（占位 → 封面）时 160ms 淡入。
///
/// 元素 id 用路径本身，列表行复用下标时换歌也不会沿用上一张图的动画状态。
pub(crate) fn thumb_img(path: PathBuf) -> AnyElement {
    let id = path.to_string_lossy().to_string();
    img(path)
        .size_full()
        .object_fit(ObjectFit::Cover)
        .with_animation(
            ElementId::Name(format!("thumb-{id}").into()),
            standard(duration::THUMB),
            |this, delta| this.opacity(delta),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// 文本交叉淡化 / 图标交叉淡化
// ---------------------------------------------------------------------------

/// 文字交叉淡化：旧文案绝对定位在上层淡出，新文案占布局并淡入 + 位移。
///
/// `enter_offset` 为正表示新内容从下方进入（下一首），为负表示从上方进入
/// （上一首）。调用方负责两段文字使用完全一致的排版样式。
pub(crate) fn crossfade_text(
    id: &str,
    seq: u64,
    old: Option<AnyElement>,
    new: AnyElement,
    duration: Duration,
    enter_offset: f32,
) -> AnyElement {
    let mut stack = div().relative().w_full();
    if let Some(old) = old {
        stack = stack.child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w_full()
                .with_animation(
                    ElementId::Name(format!("{id}-text-out-{seq}").into()),
                    standard(duration),
                    move |this, delta| {
                        this.opacity(1.0 - delta)
                            .top(px(-enter_offset * 0.4 * delta))
                    },
                )
                .child(old),
        );
    }
    stack
        .child(
            div()
                .w_full()
                .with_animation(
                    ElementId::Name(format!("{id}-text-in-{seq}").into()),
                    standard(duration),
                    move |this, delta| this.opacity(delta).top(px(enter_offset * (1.0 - delta))),
                )
                .child(new),
        )
        .into_any_element()
}

/// 图标交叉淡化：两层都绝对居中，旧图标淡出、新图标淡入并轻微缩放。
/// 父元素需要有确定的尺寸（图标容器）。
pub(crate) fn crossfade_icon(
    id: &str,
    seq: u64,
    old: Option<AnyElement>,
    new: AnyElement,
    duration: Duration,
) -> AnyElement {
    let mut stack = div().relative().size_full();
    if let Some(old) = old {
        stack = stack.child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .with_animation(
                    ElementId::Name(format!("{id}-icon-out-{seq}").into()),
                    standard(duration),
                    |this, delta| this.opacity(1.0 - delta).layer_scale(1.0 - 0.06 * delta),
                )
                .child(old),
        );
    }
    stack
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .with_animation(
                    ElementId::Name(format!("{id}-icon-in-{seq}").into()),
                    standard(duration),
                    |this, delta| this.opacity(delta).layer_scale(0.94 + 0.06 * delta),
                )
                .child(new),
        )
        .into_any_element()
}
