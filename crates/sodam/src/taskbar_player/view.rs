//! 任务栏快捷播控的播放条（与任务栏融合的轻量控件，不是卡片）。
//!
//! 只读 `Root` 的状态、把点击转发成 Root 的动作；窗口显隐由宿主层负责。
//! 不建播放引擎、不重新解析封面——封面直接用主界面已经缓存的缩略图。
//!
//! 布局：封面 + **固定 10 个汉字宽**的信息区 + 右侧控制组。信息区左对齐：
//! 短句靠左，右侧留白；超过 10 个字就横向滚动。播放时显示逐字点亮的歌词
//! （没有歌词回退歌手），暂停时显示歌名。切行时新行从下往上进入。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::AnimationExt;
use gpui::{
    div, px, svg, AnyElement, App, ClickEvent, Context, ElementId, Entity, IntoElement, Render,
    Subscription, Window,
};

use crate::app::{line_progress, lyric_tokens, Root};
use crate::ui::{icons, motion, theme};
use sodam_core::log_outlet::{log_event, LogLevel, LogTopic};

/// 播放条显隐的淡入淡出时长：任务栏里的控件要快，250ms 到位。
const FADE: Duration = Duration::from_millis(250);
/// 封面、按钮与布局间距（逻辑像素）。
const COVER: f32 = 30.0;
const CONTROL: f32 = 28.0;
const ICON: f32 = 15.0;
const BUTTON_GAP: f32 = 4.0;
const PAD: f32 = 8.0;
const GAP: f32 = 8.0;
/// 歌词/歌名字号：与播放页的大字同一档，再按 1.4 倍放大。
const LYRIC_SIZE: f32 = 18.0;
/// 信息区正好放 10 个汉字；短句左对齐，长句在这里横向滚动。
const TEXT_W: f32 = LYRIC_SIZE * 10.0;
const LINE_H: f32 = 22.0;
/// 文字整体上移一点：18px 字在 22px 行高里视觉重心偏低。
const LINE_RISE: f32 = 1.0;
/// 从封面到控制组的距离 = 两个间距 + 10 个字。
pub(crate) const BAR_WIDTH: f32 =
    PAD * 2.0 + COVER + GAP + TEXT_W + GAP + (CONTROL * 3.0 + BUTTON_GAP * 2.0);
/// 长句横向滚动速度与开始前的停顿。
const MARQUEE_SPEED: f32 = 30.0;
const MARQUEE_HOLD: f32 = 0.7;

pub(crate) struct TaskbarPlayerView {
    root: Entity<Root>,
    _root_events: Subscription,
    snapshot: Snapshot,
    /// 上一行内容、样式与当时的滚动位置，用于交叉淡化。
    line_previous: Option<LineSnapshot>,
    line_changed_at: Option<Instant>,
    line_seq: u64,
    cover: motion::CoverFade,
    /// 首帧还没读过 Root：建窗发生在 Root 的更新里，不能直接读。
    dirty: bool,
    /// 宿主通知的显隐状态；变化时做 250ms 淡入淡出。
    visible: bool,
    fade_from: f32,
    fade_seq: u64,
    fade_started: Option<Instant>,
    /// 三个按钮各自的悬停淡入淡出（160ms，和顶栏同一套）。
    hover: [motion::HoverFade; 3],
    /// 当前行实际排版宽度；超过 `TEXT_W` 才滚动。
    text_width: f32,
    measured_line: Option<String>,
    /// 当前横向滚动偏移。
    scroll: f32,
}

#[derive(Clone)]
struct LineSnapshot {
    text: String,
    playing: bool,
    width: f32,
    scroll: f32,
}

/// 渲染需要的字段快照：只有这些变了才触发过渡动画。
#[derive(Clone, Default, PartialEq)]
struct Snapshot {
    track_id: String,
    /// 单行信息：播放时歌词（无歌词回退歌手），暂停时歌名，无曲目时「未在播放」。
    line: String,
    /// 当前行唱到哪（0.0~1.0），播放中每帧更新。
    wipe: f32,
    cover: Option<PathBuf>,
    playing: bool,
}

impl TaskbarPlayerView {
    pub(crate) fn new(cx: &mut Context<Self>, root: Entity<Root>) -> Self {
        let _root_events = cx.observe(&root, |this, root, cx| {
            let next = read_snapshot(root.read(cx));
            let changed = this.dirty || next != this.snapshot;
            this.apply(next);
            if changed {
                cx.notify();
            }
        });
        Self {
            root,
            _root_events,
            snapshot: Snapshot::default(),
            line_previous: None,
            line_changed_at: None,
            line_seq: 0,
            cover: motion::CoverFade::default(),
            dirty: true,
            // 先全透明建窗；宿主把位置定准后再 set_visible(true) 淡入。
            visible: false,
            fade_from: 0.0,
            fade_seq: 1,
            fade_started: Some(Instant::now()),
            hover: [motion::HoverFade::default(); 3],
            text_width: 0.0,
            measured_line: None,
            scroll: 0.0,
        }
    }

    /// 宿主通知显隐：返回是否有变化。中断时的起点取当前进度，不跳回端点。
    pub(crate) fn set_visible(&mut self, visible: bool) -> bool {
        if self.visible == visible {
            return false;
        }
        log_event(
            LogLevel::Debug,
            LogTopic::App,
            "taskbar_view_visible",
            &[("visible", if visible { "true" } else { "false" })],
        );
        self.fade_from = self.current_opacity();
        self.visible = visible;
        self.fade_seq = self.fade_seq.wrapping_add(1);
        self.fade_started = Some(Instant::now());
        true
    }

    fn current_opacity(&self) -> f32 {
        let target = if self.visible { 1.0 } else { 0.0 };
        let Some(started) = self.fade_started else {
            return target;
        };
        let t = (started.elapsed().as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0);
        self.fade_from + (target - self.fade_from) * t
    }

    /// 应用一份新快照：只有关键字段真的变了才触发过渡。
    fn apply(&mut self, next: Snapshot) {
        if !self.dirty {
            if self.snapshot.line != next.line {
                log_event(
                    LogLevel::Debug,
                    LogTopic::App,
                    "taskbar_line",
                    &[
                        ("from", self.snapshot.line.as_str()),
                        ("to", next.line.as_str()),
                    ],
                );
            }
            if self.snapshot.playing != next.playing {
                log_event(
                    LogLevel::Debug,
                    LogTopic::App,
                    "taskbar_playing",
                    &[("playing", if next.playing { "true" } else { "false" })],
                );
            }
            if self.snapshot.cover != next.cover {
                log_event(
                    LogLevel::Debug,
                    LogTopic::App,
                    "taskbar_cover",
                    &[(
                        "has_cover",
                        if next.cover.is_some() {
                            "true"
                        } else {
                            "false"
                        },
                    )],
                );
            }
        }
        if self.dirty {
            // 首帧只启动滚动时钟，不做交叉淡化（没有上一行）。
            self.line_changed_at = Some(Instant::now());
        } else if self.snapshot.line != next.line {
            // 歌词换行、暂停/恢复共用一条交叉淡化：起点永远是当前显示的内容，
            // 所以快速切换不会闪回更早的旧行。
            self.line_previous = Some(LineSnapshot {
                text: self.snapshot.line.clone(),
                playing: self.snapshot.playing,
                width: self.text_width,
                scroll: self.scroll,
            });
            self.line_changed_at = Some(Instant::now());
            self.line_seq = self.line_seq.wrapping_add(1);
            self.scroll = 0.0;
        }
        if self.snapshot.cover != next.cover {
            self.cover.update(next.cover.clone());
        }
        self.snapshot = next;
        self.dirty = false;
    }
}

impl Render for TaskbarPlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dirty {
            // 首帧在 Root 的更新里渲染，直接读会撞 double lease；推迟一拍补数据。
            self.dirty = false;
            let this = cx.entity().downgrade();
            window.defer(cx, move |_window, cx| {
                let _ = this.update(cx, |this, cx| {
                    let next = read_snapshot(this.root.read(cx));
                    this.apply(next);
                    cx.notify();
                });
            });
        }

        // 播放中每帧读一次位置：逐字点亮和长句滚动都要跟着走。
        if self.snapshot.playing {
            let next = read_snapshot(self.root.read(cx));
            self.apply(next);
        }

        if self.snapshot.playing {
            if self.measured_line.as_deref() != Some(self.snapshot.line.as_str()) {
                self.text_width = measure_text(
                    window,
                    &self.snapshot.line,
                    LYRIC_SIZE,
                    gpui::FontWeight::NORMAL,
                );
                self.measured_line = Some(self.snapshot.line.clone());
            }
            let overflow = (self.text_width - TEXT_W).max(0.0);
            let elapsed = self
                .line_changed_at
                .map(|at| at.elapsed())
                .unwrap_or_default();
            self.scroll = marquee_offset(overflow, elapsed);
            window.request_animation_frame();
        } else {
            self.text_width = 0.0;
            self.measured_line = None;
            self.scroll = 0.0;
        }

        let playing = self.snapshot.playing;
        let has_track = !self.snapshot.track_id.is_empty();
        let light_bar = matches!(
            window.appearance(),
            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
        );
        // 首次定位前窗口是全透明的：这期间不要接收点击，
        // 否则用户点任务栏空白处可能误触一个看不见的播控。
        let interactive = has_track && self.visible;
        let cover = self.cover_layers(light_bar);
        let line = self.line_layer(window, light_bar);

        let controls = div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(BUTTON_GAP))
            .child(self.button(
                0,
                "taskbar-prev",
                "skip-back",
                !interactive,
                light_bar,
                cx,
                |root, cx| root.prev_track(cx),
            ))
            .child(self.button(
                1,
                "taskbar-toggle",
                if playing { "pause" } else { "play" },
                !interactive,
                light_bar,
                cx,
                |root, cx| root.toggle_play(cx),
            ))
            .child(self.button(
                2,
                "taskbar-next",
                "skip-forward",
                !interactive,
                light_bar,
                cx,
                |root, cx| root.next_track(cx),
            ));

        // 封面 + 固定 10 字宽的信息区：短句靠左，右侧留白，长句内部滚动。
        let open_area = div()
            .id("taskbar-open")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(GAP))
            .flex_none()
            .cursor_pointer()
            .on_click(cx.listener(|this, _event: &ClickEvent, _window, cx| {
                if this.visible {
                    this.activate_main(cx);
                }
            }))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(px(COVER))
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .children(cover),
            )
            .child(div().flex_none().w(px(TEXT_W)).h(px(LINE_H)).child(line));

        let bar = div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(GAP))
            .px(px(PAD))
            .child(open_area)
            .child(controls);

        // 显隐淡入淡出：窗口本身由宿主显示/隐藏，这里只动内容透明度。
        let target = if self.visible { 1.0 } else { 0.0 };
        match self.fade_started {
            Some(started) if started.elapsed() < FADE => {
                let from = self.fade_from;
                bar.with_animation(
                    ElementId::Name(format!("taskbar-fade-{}", self.fade_seq).into()),
                    motion::standard(FADE),
                    move |this, delta| this.opacity(from + (target - from) * delta),
                )
                .into_any_element()
            }
            _ => bar.opacity(target).into_any_element(),
        }
    }
}

impl TaskbarPlayerView {
    fn cover_layers(&self, light_bar: bool) -> Vec<AnyElement> {
        let placeholder_bg = if light_bar {
            theme::rgba(0, 0, 0, 0.06)
        } else {
            theme::rgba(255, 255, 255, 0.08)
        };
        let placeholder_fg = if light_bar {
            theme::rgba(18, 18, 22, 0.42)
        } else {
            theme::rgba(255, 255, 255, 0.40)
        };
        let placeholder = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(placeholder_bg)
            .child(
                svg()
                    .path(icons::path("music"))
                    .size(px(14.0))
                    .text_color(placeholder_fg),
            )
            .into_any_element();
        motion::cover_layers(
            "taskbar-cover",
            &self.cover,
            motion::duration::COVER_SMALL,
            4.0,
            placeholder,
        )
    }

    fn line_layer(&self, window: &Window, light_bar: bool) -> AnyElement {
        let fresh = line_element(
            window,
            &self.snapshot.line,
            self.snapshot.playing,
            self.snapshot.wipe,
            self.text_width,
            self.scroll,
            light_bar,
        );
        let animating = self
            .line_changed_at
            .is_some_and(|at| at.elapsed() < motion::duration::TASKBAR_LYRIC);
        match (animating, self.line_previous.as_ref()) {
            (true, Some(previous)) => {
                let old = line_element(
                    window,
                    &previous.text,
                    previous.playing,
                    // 旧歌词退场时保持整行点亮，和播放页一致。
                    1.0,
                    previous.width,
                    previous.scroll,
                    light_bar,
                );
                motion::crossfade_text(
                    "taskbar-line",
                    self.line_seq,
                    Some(old),
                    fresh,
                    motion::duration::TASKBAR_LYRIC,
                    10.0,
                )
            }
            _ => fresh.into_any_element(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        slot: usize,
        id: &'static str,
        icon: &'static str,
        disabled: bool,
        light_bar: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Root, &mut Context<Root>) + 'static,
    ) -> AnyElement {
        let color = match (disabled, light_bar) {
            (true, true) => theme::rgba(18, 18, 22, 0.35),
            (true, false) => theme::rgba(255, 255, 255, 0.38),
            (false, true) => theme::rgba(18, 18, 22, 0.72),
            (false, false) => theme::rgba(255, 255, 255, 0.72),
        };
        let hover_bg = if light_bar {
            theme::rgba(0, 0, 0, 0.08)
        } else {
            theme::rgba(255, 255, 255, 0.12)
        };
        let mut button = div()
            .id(id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(CONTROL))
            .rounded(px(6.0));
        if !disabled {
            button = button
                .cursor_pointer()
                .child(motion::hover_layer(id, self.hover[slot], 6.0, hover_bg))
                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                    let fade = &mut this.hover[slot];
                    if fade.hovered != *hovered {
                        fade.hovered = *hovered;
                        fade.seq = fade.seq.wrapping_add(1);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                    this.root.update(cx, |root, cx| action(root, cx));
                }));
        }
        button
            .child(
                svg()
                    .path(icons::path(icon))
                    .size(px(ICON))
                    .text_color(color),
            )
            .into_any_element()
    }

    /// 点播放条 = 打开/激活主窗口；主窗口关掉后也能重新建出来。
    fn activate_main(&self, cx: &mut App) {
        crate::show_window(cx, &self.root);
    }
}

/// 单行内容：播放时是逐字染色的歌词，暂停时是歌名。
fn line_element(
    window: &Window,
    text: &str,
    playing: bool,
    wipe: f32,
    width: f32,
    scroll: f32,
    light_bar: bool,
) -> AnyElement {
    let fg_primary = if light_bar {
        theme::rgba(18, 18, 22, 0.92)
    } else {
        theme::rgba(255, 255, 255, 0.94)
    };
    let fg_muted = if light_bar {
        theme::rgba(18, 18, 22, 0.56)
    } else {
        theme::rgba(255, 255, 255, 0.62)
    };
    if !playing {
        return div()
            .relative()
            .top(px(-LINE_RISE))
            .w(px(TEXT_W))
            .h(px(LINE_H))
            .flex()
            .items_center()
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_size(px(LYRIC_SIZE))
                    .line_height(px(LINE_H))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(fg_primary)
                    .child(text.to_string()),
            )
            .into_any_element();
    }

    let width = if width > 0.0 {
        width
    } else {
        measure_text(window, text, LYRIC_SIZE, gpui::FontWeight::NORMAL)
    };
    let tokens = lyric_tokens(text);
    let total: f32 = tokens
        .iter()
        .map(|token| token.weight)
        .sum::<f32>()
        .max(1.0);
    let mut cumulative = 0.0;
    let mut row = div()
        .absolute()
        .top(px(0.0))
        .left(px(-scroll))
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .w(px(width))
        .text_size(px(LYRIC_SIZE))
        .line_height(px(LINE_H))
        .font_weight(gpui::FontWeight::NORMAL);
    for token in tokens {
        let start = cumulative / total;
        cumulative += token.weight;
        let end = cumulative / total;
        let raw = ((wipe - start) / (end - start).max(1e-6)).clamp(0.0, 1.0);
        // smoothstep：和播放页同一套逐字点亮曲线。
        let eased = raw * raw * (3.0 - 2.0 * raw);
        row = row.child(
            div()
                .flex_none()
                .text_color(motion::mix_rgba(fg_muted, fg_primary, eased))
                .child(token.text),
        );
    }
    div()
        .relative()
        .top(px(-LINE_RISE))
        .w(px(TEXT_W))
        .h(px(LINE_H))
        .overflow_hidden()
        .child(row)
        .into_any_element()
}

fn measure_text(window: &Window, text: &str, size: f32, weight: gpui::FontWeight) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let style = window.text_style();
    let mut run = style.to_run(text.len());
    run.font.weight = weight;
    f32::from(
        window
            .text_system()
            .layout_line(text, px(size), &[run], None)
            .width,
    )
}

/// 长句横向滚动：停一下，然后单向滑到末尾并保持，不再回滚。
fn marquee_offset(overflow: f32, elapsed: Duration) -> f32 {
    if overflow <= 0.0 {
        return 0.0;
    }
    ((elapsed.as_secs_f32() - MARQUEE_HOLD).max(0.0) * MARQUEE_SPEED).min(overflow)
}

fn read_snapshot(root: &Root) -> Snapshot {
    let engine = root.engine.snapshot();
    let track = root.queue.current();
    let artist = track.map(|track| track.artist.clone()).unwrap_or_default();
    let title = if engine.title.is_empty() {
        track.map(|track| track.title.clone()).unwrap_or_default()
    } else {
        engine.title.clone()
    };
    let (line, wipe) = if engine.track_id.is_empty() {
        (root.tr("未在播放").to_string(), 1.0)
    } else if !engine.playing {
        (title, 1.0)
    } else {
        let position_ms = (root.display_position_seconds().max(0.0) * 1000.0).round() as u64;
        let active = root.lyrics.as_ref().and_then(|lyrics| {
            let index = lyrics.active_index(position_ms)?;
            let text = lyrics.lines.get(index)?.text.trim();
            if text.is_empty() {
                return None;
            }
            Some((text.to_string(), line_progress(lyrics, index, position_ms)))
        });
        active.unwrap_or_else(|| {
            // 间奏行没有歌词、歌手标签也可能为空；这时不要留一条空白，
            // 退回歌名，至少还能看出在放哪首。
            if artist.trim().is_empty() {
                (title.clone(), 1.0)
            } else {
                (artist, 1.0)
            }
        })
    };
    Snapshot {
        track_id: engine.track_id.clone(),
        line,
        wipe,
        cover: root.player_bar_cover(),
        playing: engine.playing,
    }
}

#[cfg(test)]
mod tests {
    use super::{marquee_offset, MARQUEE_HOLD, MARQUEE_SPEED};
    use std::time::Duration;

    #[test]
    fn marquee_holds_then_scrolls_once_and_stays() {
        let overflow = 36.0;
        assert_eq!(marquee_offset(0.0, Duration::from_secs(2)), 0.0);
        assert_eq!(marquee_offset(overflow, Duration::from_secs(0)), 0.0);

        let travel = overflow / MARQUEE_SPEED;
        assert_eq!(
            marquee_offset(overflow, Duration::from_secs_f32(MARQUEE_HOLD)),
            0.0
        );
        let at_end = Duration::from_secs_f32(MARQUEE_HOLD + travel);
        assert!((marquee_offset(overflow, at_end) - overflow).abs() < 0.01);
        // 到末尾后不再回滚。
        let later = Duration::from_secs_f32(MARQUEE_HOLD + travel + 5.0);
        assert!((marquee_offset(overflow, later) - overflow).abs() < 0.01);
    }
}
