//! 底部播放条 + 右侧队列抽屉。
//!
//! 对齐汽水音乐：高 112、实底 base-7；左侧 68 封面（点击展开大封面与歌词）、
//! 中间是 100×56 的播放胶囊，进度条是贯穿整窗顶部的 2px 细线，右侧是模式/音量/队列。

use gpui::prelude::*;
use gpui::{
    canvas, div, img, px, svg, uniform_list, AnimationExt, AnyElement, ClickEvent, Context,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit,
};

use crate::app::{playing_strength, Root};
use crate::ui::motion::{self, HoverFade};
use crate::ui::{icons, theme};

/// 内联音量条宽度与滑块直径。
const VOLUME_TRACK_W: f32 = 96.0;
const VOLUME_THUMB: f32 = 10.0;

/// 小型图标按钮：播放条/抽屉的通用探头。hover 120ms 淡入，点击回调由调用方挂。
fn icon_button(
    id: &'static str,
    icon: &'static str,
    size: f32,
    color: gpui::Rgba,
    fade: HoverFade,
) -> gpui::Stateful<gpui::Div> {
    icon_button_content(
        id,
        fade,
        svg()
            .path(icons::path(icon))
            .size(px(size))
            .text_color(color),
    )
}

/// 图标按钮外壳：内容由调用方给（静态图标或交叉淡化的图标栈）。
fn icon_button_content(
    id: &'static str,
    fade: HoverFade,
    content: impl IntoElement + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .size(px(40.0))
        .cursor_pointer()
        .rounded(px(theme::radius::ROW))
        .child(motion::hover_layer(
            id,
            fade,
            theme::radius::ROW,
            theme::surface_hover(),
        ))
        .active(|style| style.layer_scale(0.96))
        .child(content)
}

fn transport_button(
    id: &'static str,
    icon: &'static str,
    fade: HoverFade,
    cx: &mut Context<Root>,
    action: fn(&mut Root, &mut Context<Root>),
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .w(px(64.0))
        .h(px(theme::size::PLAY_BTN_H))
        .rounded(px(theme::radius::PILL))
        .cursor_pointer()
        .child(motion::hover_layer(
            id,
            fade,
            theme::radius::PILL,
            theme::surface_hover(),
        ))
        .active(|style| style.layer_scale(0.96))
        .on_hover(Root::hover_listener(cx, id))
        .on_click(cx.listener(move |root, _event: &ClickEvent, _window, cx| {
            action(root, cx);
            cx.notify();
        }))
        .child(
            svg()
                .path(icons::path(icon))
                .size(px(theme::ICON))
                .text_color(theme::text_muted()),
        )
}

/// 等价的静态占位图：本地文件没有封面时显示音符。
fn track_art(size: f32, radius: f32) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(radius))
        .bg(theme::surface_elevated())
        .border_1()
        .border_color(theme::border())
        .child(
            svg()
                .path(icons::path("music"))
                .size(px(theme::ICON_SM))
                .text_color(theme::text_faint()),
        )
}

/// 播放/暂停图标：状态切换时交叉淡化，而不是直接换图。
fn play_icon(root: &Root, playing: bool) -> AnyElement {
    let icon = |name: &'static str| -> AnyElement {
        svg()
            .path(icons::path(name))
            .size(px(theme::ICON_LG))
            .text_color(gpui::white())
            .into_any_element()
    };
    let current = if playing { "pause" } else { "play" };
    if root.play_icon_seq == 0 {
        return icon(current);
    }
    let previous = if playing { "play" } else { "pause" };
    motion::crossfade_icon(
        "play-icon",
        root.play_icon_seq,
        Some(icon(previous)),
        icon(current),
        motion::duration::ICON,
    )
}

/// 正在播放的封面；点击展开「正在播放」页。
fn cover_thumb(root: &mut Root, cx: &mut Context<Root>) -> impl IntoElement {
    let size = theme::size::PLAYER_THUMB;
    let cover = root
        .player_bar_cover()
        .or_else(|| root.playlist_art.as_ref().map(|art| art.cover_path.clone()));
    // 切歌时双层交叉淡化：旧封面在下、新封面 240ms 淡入。
    root.bar_cover_fade.update(cover);
    let placeholder = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::surface_elevated())
        .child(
            svg()
                .path(icons::path("music"))
                .size(px(theme::ICON))
                .text_color(theme::text_faint()),
        )
        .into_any_element();
    let layers = motion::cover_layers(
        "bar-cover",
        &root.bar_cover_fade,
        motion::duration::COVER_SMALL,
        4.0,
        placeholder,
    );
    let hover_fade = root.hovers.get("player-cover-hover");
    let overlay = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000099))
        .child(
            svg()
                // 播放页里同一个位置是「收起」，箭头方向反过来。
                .path(icons::path(if root.player_view_open {
                    "collapse"
                } else {
                    "expand"
                }))
                .size(px(theme::ICON))
                .text_color(gpui::white()),
        );
    let overlay = if hover_fade.seq == 0 {
        overlay
            .opacity(if hover_fade.hovered { 1.0 } else { 0.0 })
            .into_any_element()
    } else {
        let (from, to) = if hover_fade.hovered {
            (0.0, 1.0)
        } else {
            (1.0, 0.0)
        };
        overlay
            .with_animation(
                gpui::ElementId::Name(format!("player-cover-hover-{}", hover_fade.seq).into()),
                motion::standard(motion::duration::HOVER),
                move |this, delta| this.opacity(from + (to - from) * delta),
            )
            .into_any_element()
    };
    let mut element = div()
        .id("player-cover")
        .relative()
        .flex_none()
        .size(px(size))
        .rounded(px(4.0))
        .overflow_hidden()
        .cursor_pointer()
        .on_hover(Root::hover_listener(cx, "player-cover-hover"))
        .on_click(cx.listener(|root, _event: &ClickEvent, _window, cx| {
            if root.player_view_open {
                root.close_player_view(cx);
            } else {
                root.open_player_view(cx);
            }
        }));
    for layer in layers {
        element = element.child(layer);
    }
    element.child(overlay)
}

/// 内联音量滑条：细轨道 + 主题色已播段 + 圆形滑块，点或拖都能调。
fn volume_slider(root: &Root, cx: &mut Context<Root>) -> impl IntoElement {
    let volume = root.settings.volume;
    let filled = (VOLUME_TRACK_W - VOLUME_THUMB) * volume;
    let bounds_slot = root.volume_track_bounds.clone();
    let apply = move |x: f32, root: &mut Root, cx: &mut Context<Root>| {
        let left = bounds_slot
            .lock()
            .ok()
            .and_then(|value| value.map(|rect| f32::from(rect.left())))
            .unwrap_or(0.0);
        let ratio = ((x - left) / VOLUME_TRACK_W).clamp(0.0, 1.0);
        root.set_volume(ratio);
        cx.notify();
    };
    let apply_down = apply.clone();
    let apply_move = apply.clone();
    let slot_for_canvas = root.volume_track_bounds.clone();

    div()
        .relative()
        .flex()
        .items_center()
        .h(px(16.0))
        .w(px(VOLUME_TRACK_W))
        .flex_none()
        .child(
            div()
                .absolute()
                .top(px(6.0))
                .left(px(0.0))
                .w(px(VOLUME_TRACK_W))
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(theme::surface_hover()),
        )
        .child(
            div()
                .absolute()
                .top(px(6.0))
                .left(px(0.0))
                .w(px(filled + VOLUME_THUMB / 2.0))
                .h(px(4.0))
                .rounded(px(2.0))
                .bg(theme::progress_fill()),
        )
        .child(
            div()
                .absolute()
                .top(px((16.0 - VOLUME_THUMB) / 2.0))
                .left(px(filled))
                .size(px(VOLUME_THUMB))
                .rounded_full()
                .bg(theme::text()),
        )
        .child(
            canvas(
                move |bounds: gpui::Bounds<gpui::Pixels>, _window, _cx| {
                    if let Ok(mut guard) = slot_for_canvas.lock() {
                        *guard = Some(bounds);
                    }
                },
                |_bounds, _state, _window, _cx| {},
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .id("volume-track")
                .absolute()
                .size_full()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |root, event: &MouseDownEvent, _window, cx| {
                        apply_down(f32::from(event.position.x), root, cx);
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |root, event: &MouseMoveEvent, _window, cx| {
                        if event.pressed_button == Some(MouseButton::Left) {
                            apply_move(f32::from(event.position.x), root, cx);
                        }
                    }),
                ),
        )
}

/// 顶部 2px 进度线：贯穿整个窗口，hover 时露出白色圆点。
///
/// 拖动过程中只更新预览（不 seek），**松手才真正跳转**。
fn top_progress(
    root: &Root,
    progress: f32,
    loading: bool,
    hover_fade: HoverFade,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let shown = root.progress_preview.unwrap_or(progress).clamp(0.0, 1.0);
    let bounds_slot = root.progress_track_bounds.clone();
    let fraction_at = move |x: f32| -> Option<f32> {
        let (left, width) = bounds_slot.lock().ok().and_then(|value| {
            value.map(|rect| (f32::from(rect.left()), f32::from(rect.size.width)))
        })?;
        if width <= 1.0 {
            return None;
        }
        Some(((x - left) / width).clamp(0.0, 1.0))
    };
    let fraction_for_down = fraction_at.clone();
    let fraction_for_move = fraction_at.clone();
    let slot_for_canvas = root.progress_track_bounds.clone();

    let dot = div()
        .absolute()
        .top(px(-4.0))
        .left(gpui::relative(shown))
        .size(px(10.0))
        .rounded_full()
        .bg(theme::text());
    // 悬停圆点 100ms 淡入、离开 120ms 淡出；拖动时位置仍然完全跟手。
    let dot = if hover_fade.seq == 0 {
        dot.opacity(if hover_fade.hovered { 1.0 } else { 0.0 })
            .into_any_element()
    } else {
        let (from, to) = if hover_fade.hovered {
            (0.0, 1.0)
        } else {
            (1.0, 0.0)
        };
        dot.with_animation(
            gpui::ElementId::Name(format!("progress-dot-{}", hover_fade.seq).into()),
            motion::standard(motion::duration::HOVER),
            move |this, delta| this.opacity(from + (to - from) * delta),
        )
        .into_any_element()
    };

    div()
        .id("player-progress")
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .right(px(0.0))
        .h(px(12.0))
        .cursor_pointer()
        .on_hover(Root::hover_listener(cx, "player-progress"))
        .when(loading, |this| this.opacity(0.4))
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .h(px(2.0))
                .bg(theme::border()),
        )
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .h(px(2.0))
                .rounded(px(2.0))
                .bg(theme::progress_fill())
                .w(gpui::relative(shown)),
        )
        .child(dot)
        .child(canvas(
            move |bounds: gpui::Bounds<gpui::Pixels>, _window, _cx| {
                if let Ok(mut guard) = slot_for_canvas.lock() {
                    *guard = Some(bounds);
                }
            },
            |_bounds, _state, _window, _cx| {},
        ))
        .child(
            div()
                .id("progress-track")
                .absolute()
                .size_full()
                .when(!loading, |this| this.cursor_pointer())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |root, event: &MouseDownEvent, _window, cx| {
                        if root.pending_track.is_some() {
                            return;
                        }
                        if let Some(fraction) = fraction_for_down(f32::from(event.position.x)) {
                            root.progress_preview = Some(fraction);
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |root, event: &MouseMoveEvent, _window, cx| {
                        if root.pending_track.is_some()
                            || event.pressed_button != Some(MouseButton::Left)
                        {
                            return;
                        }
                        if let Some(fraction) = fraction_for_move(f32::from(event.position.x)) {
                            root.progress_preview = Some(fraction);
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |root, _event: &gpui::MouseUpEvent, _window, cx| {
                        if let Some(fraction) = root.progress_preview.take() {
                            if root.pending_track.is_none() {
                                root.seek_fraction(fraction);
                            }
                            cx.notify();
                        }
                    }),
                )
                // 在进度条外松开鼠标时 on_mouse_up 不会触发：补一条，
                // 否则预览值永久保留、进度条冻结且这次 seek 丢失。
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(move |root, _event: &gpui::MouseUpEvent, _window, cx| {
                        if let Some(fraction) = root.progress_preview.take() {
                            if root.pending_track.is_none() {
                                root.seek_fraction(fraction);
                            }
                            cx.notify();
                        }
                    }),
                ),
        )
}

/// 渲染播放条。
pub fn render(root: &mut Root, cx: &mut Context<Root>) -> impl IntoElement {
    let language = root.language;
    let snapshot = root.engine.snapshot();
    let loading = root.pending_track.is_some();
    // 加载中显示「正在准备的这首」，而不是还在响的上一首。
    let track = root
        .pending_track
        .clone()
        .or_else(|| root.queue.current().cloned());
    let title = track
        .as_ref()
        .map(|track| track.title.clone())
        .unwrap_or_else(|| language.text("未在播放").to_string());
    let artist = track
        .as_ref()
        .map(|track| track.artist.clone())
        .unwrap_or_else(|| language.text("选择一首歌开始播放吧").to_string());
    let album = track
        .as_ref()
        .map(|track| track.album.clone())
        .unwrap_or_default();
    // 切歌时标题/歌手做 180ms 交叉淡化，方向由上一首/下一首决定。
    root.note_track_text(&title, &artist, &album);
    let text_swap = root
        .track_text_swap
        .as_ref()
        .filter(|swap| swap.active())
        .map(|swap| {
            (
                swap.from_title.clone(),
                swap.from_artist.clone(),
                swap.seq,
                swap.direction,
            )
        });
    let error = if !snapshot.error.is_empty() {
        language.engine_error(&snapshot.error)
    } else if let Some(text) = root.playback_error.as_deref() {
        language.engine_error(text)
    } else {
        String::new()
    };
    // 以插值后的位置画进度：快照只约 10Hz，直接用它会让进度条一跳一跳。
    let display_position = root.display_position_seconds();
    let progress = if snapshot.duration_seconds > 0.0 {
        (display_position / snapshot.duration_seconds).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    // 播放/暂停、静音、播放模式图标：状态切换时交叉淡化（140ms）。
    let playing = snapshot.playing;
    if root.play_icon_shown != playing {
        root.play_icon_shown = playing;
        root.play_icon_seq = root.play_icon_seq.wrapping_add(1);
    }
    let muted = root.settings.volume <= 0.01;
    if root.mute_icon_shown != muted {
        root.mute_icon_shown = muted;
        root.mute_icon_seq = root.mute_icon_seq.wrapping_add(1);
    }
    let mode_icon = root.queue.mode.icon();
    let mut mode_previous = None;
    if root.mode_icon_shown != mode_icon {
        mode_previous = Some(root.mode_icon_shown);
        root.mode_icon_shown = mode_icon;
        root.mode_icon_seq = root.mode_icon_seq.wrapping_add(1);
    }
    let mode_seq = root.mode_icon_seq;
    let volume = root.settings.volume;
    let can_open_player = track.is_some();
    let progress_fade = root.hovers.get("player-progress");
    let prev_fade = root.hovers.get("prev");
    let next_fade = root.hovers.get("next");
    let play_fade = root.hovers.get("play-toggle");
    let lyrics_fade = root.hovers.get("player-lyrics");
    let mode_fade = root.hovers.get("play-mode");
    let volume_fade = root.hovers.get("volume");
    let queue_fade = root.hovers.get("queue");

    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(theme::PLAYER_H))
        .w_full()
        .flex_none()
        .px(px(theme::space::XL))
        // 半透明磨砂底：整页色板变化会透到这一层。
        .bg(theme::chrome())
        .child(top_progress(root, progress, loading, progress_fade, cx))
        // 左：封面 + 标题/歌手
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::space::LG))
                .w(px(360.0))
                .min_w(px(220.0))
                .flex_shrink(1.0)
                .child(if can_open_player {
                    cover_thumb(root, cx).into_any_element()
                } else {
                    track_art(theme::size::PLAYER_THUMB, 4.0).into_any_element()
                })
                .child(
                    div()
                        .id("player-track-info")
                        .flex()
                        .flex_col()
                        .min_w(px(0.0))
                        .flex_1()
                        .when(can_open_player, |this| {
                            this.cursor_pointer().on_click(cx.listener(
                                |root, _event: &ClickEvent, _window, cx| {
                                    root.open_player_view(cx);
                                },
                            ))
                        })
                        .child({
                            let style = |text: String| -> AnyElement {
                                div()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(theme::Text::Large.size())
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme::text())
                                    .child(text)
                                    .into_any_element()
                            };
                            let new = style(title.clone());
                            match &text_swap {
                                Some((from, _, seq, direction)) => motion::crossfade_text(
                                    "bar-title",
                                    *seq,
                                    Some(style(from.clone())),
                                    new,
                                    motion::duration::CONTENT,
                                    4.0 * direction,
                                ),
                                None => new,
                            }
                        })
                        .child({
                            let style = |text: String| -> AnyElement {
                                div()
                                    .w_full()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(theme::Text::Small.size())
                                    .text_color(theme::text_muted())
                                    .child(text)
                                    .into_any_element()
                            };
                            let new = style(artist.clone());
                            match &text_swap {
                                Some((_, from, seq, direction)) => motion::crossfade_text(
                                    "bar-artist",
                                    *seq,
                                    Some(style(from.clone())),
                                    new,
                                    motion::duration::CONTENT,
                                    4.0 * direction,
                                ),
                                None => new,
                            }
                        })
                        .when(!error.is_empty(), |this| {
                            this.child(
                                div()
                                    .truncate()
                                    .text_size(theme::Text::Tiny.size())
                                    .text_color(theme::danger())
                                    .child(error),
                            )
                        }),
                ),
        )
        // 中：时间 + 传输控制
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(theme::space::XL))
                .flex_1()
                .child(
                    div()
                        .flex_none()
                        .text_size(theme::Text::Small.size())
                        .text_color(theme::text_faint())
                        .child(format!(
                            "{} / {}",
                            sodam_core::audio::format_seconds(display_position),
                            snapshot.duration_label()
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::space::SM))
                        .child(transport_button(
                            "prev",
                            "skip-back",
                            prev_fade,
                            cx,
                            |root, cx| {
                                root.prev_track(cx);
                            },
                        ))
                        .child(
                            div().relative().child(
                                div()
                                    .id("play-toggle")
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(theme::size::PLAY_BTN_W))
                                    .h(px(theme::size::PLAY_BTN_H))
                                    .rounded(px(theme::radius::PILL))
                                    .bg(gpui::rgba(0xFFFFFF0D))
                                    .cursor_pointer()
                                    .child(motion::hover_layer(
                                        "play-toggle",
                                        play_fade,
                                        theme::radius::PILL,
                                        gpui::rgba(0xFFFFFF1A),
                                    ))
                                    .active(|style| style.layer_scale(0.97))
                                    .on_hover(Root::hover_listener(cx, "play-toggle"))
                                    .on_click(cx.listener(
                                        |root, _event: &ClickEvent, _window, cx| {
                                            root.toggle_play(cx);
                                            cx.notify();
                                        },
                                    ))
                                    .child(if loading {
                                        crate::ui::spinner::spinner(theme::ICON_LG, theme::text())
                                            .into_any_element()
                                    } else {
                                        // 播放/暂停图标 140ms 交叉淡化 + 轻微缩放。
                                        div()
                                            .relative()
                                            .size(px(theme::ICON_LG))
                                            .child(play_icon(root, playing))
                                            .into_any_element()
                                    }),
                            ),
                        )
                        .child(transport_button(
                            "next",
                            "skip-forward",
                            next_fade,
                            cx,
                            |root, cx| {
                                root.next_track(cx);
                            },
                        )),
                ),
        )
        // 右：歌词入口 + 播放模式 + 音量 + 队列
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(theme::space::SM))
                .w(px(330.0))
                .min_w(px(250.0))
                .flex_shrink(1.0)
                .child(
                    div()
                        .id("player-lyrics")
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(theme::size::CONTROL))
                        .px(px(theme::space::SM))
                        .rounded(px(theme::radius::ROW))
                        .cursor_pointer()
                        .child(motion::hover_layer(
                            "player-lyrics",
                            lyrics_fade,
                            theme::radius::ROW,
                            theme::surface_hover(),
                        ))
                        .active(|style| style.layer_scale(0.96))
                        .on_hover(Root::hover_listener(cx, "player-lyrics"))
                        .on_click(cx.listener(|root, _event: &ClickEvent, _window, cx| {
                            if root.player_view_open {
                                root.close_player_view(cx);
                            } else {
                                root.open_player_view(cx);
                            }
                        }))
                        .text_size(px(16.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        // 播放页打开时用强调色，让「已展开」这件事在播放栏也有反馈。
                        .text_color(if root.player_view_open {
                            theme::accent()
                        } else {
                            theme::text_muted()
                        })
                        .child(root.tr("词")),
                )
                .child(
                    icon_button_content(
                        "play-mode",
                        mode_fade,
                        div().relative().size(px(theme::ICON)).child({
                            if mode_seq == 0 {
                                svg()
                                    .path(icons::path(mode_icon))
                                    .size(px(theme::ICON))
                                    .text_color(theme::text_muted())
                                    .into_any_element()
                            } else {
                                let old = mode_previous.map(|previous| {
                                    svg()
                                        .path(icons::path(previous))
                                        .size(px(theme::ICON))
                                        .text_color(theme::text_muted())
                                        .into_any_element()
                                });
                                motion::crossfade_icon(
                                    "mode-icon",
                                    mode_seq,
                                    old,
                                    svg()
                                        .path(icons::path(mode_icon))
                                        .size(px(theme::ICON))
                                        .text_color(theme::text_muted())
                                        .into_any_element(),
                                    motion::duration::ICON,
                                )
                            }
                        }),
                    )
                    .on_hover(Root::hover_listener(cx, "play-mode"))
                    .on_click(cx.listener(
                        |root, _event: &ClickEvent, _window, cx| {
                            root.cycle_play_mode(cx);
                        },
                    )),
                )
                .child(
                    icon_button_content(
                        "volume",
                        volume_fade,
                        div().relative().size(px(theme::ICON)).child({
                            let icon_name = if volume <= 0.01 {
                                "volume-x"
                            } else {
                                "volume-2"
                            };
                            if root.mute_icon_seq == 0 {
                                svg()
                                    .path(icons::path(icon_name))
                                    .size(px(theme::ICON))
                                    .text_color(theme::text_muted())
                                    .into_any_element()
                            } else {
                                let old = svg()
                                    .path(icons::path(if volume <= 0.01 {
                                        "volume-2"
                                    } else {
                                        "volume-x"
                                    }))
                                    .size(px(theme::ICON))
                                    .text_color(theme::text_muted())
                                    .into_any_element();
                                motion::crossfade_icon(
                                    "mute-icon",
                                    root.mute_icon_seq,
                                    Some(old),
                                    svg()
                                        .path(icons::path(icon_name))
                                        .size(px(theme::ICON))
                                        .text_color(theme::text_muted())
                                        .into_any_element(),
                                    motion::duration::ICON,
                                )
                            }
                        }),
                    )
                    .on_hover(Root::hover_listener(cx, "volume"))
                    .on_click(cx.listener(
                        |root, _event: &ClickEvent, _window, cx| {
                            root.toggle_mute();
                            cx.notify();
                        },
                    )),
                )
                .child(volume_slider(root, cx))
                .child(
                    icon_button(
                        "queue",
                        "list-end",
                        theme::ICON,
                        theme::text_muted(),
                        queue_fade,
                    )
                    .on_hover(Root::hover_listener(cx, "queue"))
                    .on_click(cx.listener(
                        |root, _event: &ClickEvent, _window, cx| {
                            root.toggle_queue(cx);
                        },
                    )),
                ),
        )
}

// ---------------------------------------------------------------------------
// 右侧队列抽屉（覆盖在内容之上，宽 420）
// ---------------------------------------------------------------------------

/// 抽屉里的一行：分区标题或曲目。用**同一个高度**排布，
/// 这样可以用 `uniform_list` 虚拟化（只渲染可见的十来行）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum QueueSlot {
    History(usize),
    NowPlaying,
    Upcoming(usize),
    Track { index: usize, playing: bool },
}

/// 队列抽屉行高（虚拟列表要求等高）。
const QUEUE_ROW_H: f32 = 64.0;

/// 队列侧板宽度；`app` 里的滑入/滑出动画和滚轮区域判断共用。
pub const QUEUE_WIDTH: f32 = 420.0;

fn queue_slots(root: &Root) -> Vec<QueueSlot> {
    let tracks = root.queue.tracks();
    let current = root.queue.index();
    let upcoming = root.queue.upcoming();
    // 「接下来」里已经列出的曲目不再进「已播放」段：往后翻过再退回时，
    // 同一首会既算历史又算接下来，抽屉里出现两行。
    let upcoming_set: std::collections::HashSet<usize> = upcoming.iter().copied().collect();
    let mut slots = Vec::new();

    if !root.played_history.is_empty() {
        // 同一首可能被反复播放（列表循环两圈），「已播放」里要按曲目去重，
        // 否则它会出现两行。
        let mut seen = std::collections::HashSet::new();
        let history: Vec<usize> = root
            .played_history
            .iter()
            .filter_map(|id| tracks.iter().position(|track| &track.id == id))
            .filter(|index| *index != current && !upcoming_set.contains(index))
            .filter(|index| seen.insert(*index))
            .collect();
        if !history.is_empty() {
            slots.push(QueueSlot::History(history.len()));
            for index in history {
                slots.push(QueueSlot::Track {
                    index,
                    playing: false,
                });
            }
        }
    }
    if current < tracks.len() {
        slots.push(QueueSlot::NowPlaying);
        slots.push(QueueSlot::Track {
            index: current,
            playing: true,
        });
    }
    if !upcoming.is_empty() {
        slots.push(QueueSlot::Upcoming(upcoming.len()));
        for index in upcoming {
            if index < tracks.len() {
                slots.push(QueueSlot::Track {
                    index,
                    playing: false,
                });
            }
        }
    }
    slots
}

/// 「正在播放」所在的行号（打开抽屉时锚定到它）。
pub(crate) fn now_playing_slot(root: &Root) -> usize {
    queue_slots(root)
        .iter()
        .position(|slot| matches!(slot, QueueSlot::NowPlaying))
        .unwrap_or(0)
}

/// 某首曲目在抽屉虚拟列表里的槽位（删除时定位幽灵行）。
pub(crate) fn slot_of_track(root: &Root, track_index: usize) -> Option<usize> {
    queue_slots(root)
        .iter()
        .position(|slot| matches!(slot, QueueSlot::Track { index, .. } if *index == track_index))
}

fn queue_section_row(title: &str) -> gpui::AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .h(px(QUEUE_ROW_H))
        .px(px(theme::space::XL))
        .text_size(theme::Text::Tiny.size())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme::text_faint())
        .child(title.to_string())
        .into_any_element()
}

/// 主界面的右侧队列抽屉。
pub(crate) fn queue_drawer(root: &mut Root, cx: &mut Context<Root>) -> impl IntoElement {
    use std::sync::Arc;

    let language = root.language;
    let slots = Arc::new(queue_slots(root));
    let tracks = root.queue_cache.1.clone();
    let entity = cx.entity();
    let count = slots.len();
    let total = root.queue.len();
    let now_cover = root.artwork.as_ref().map(|art| art.cover_path.clone());
    let covers = root.cover_thumbs.clone();
    let ghost_covers = covers.clone();
    let hover_state = root.queue_hover;
    let np_current = root.now_playing_fade.current.clone();
    let np_previous = root.now_playing_fade.previous.clone();
    let np_t = root.now_playing_fade.progress();
    // 删除后的退出/归位动画只活 240ms；过期后不再渲染幽灵层。
    let recent_removal = root
        .queue_removal
        .as_ref()
        .filter(|removal| removal.at.elapsed() < std::time::Duration::from_millis(240));
    let reflow = recent_removal.map(|removal| (removal.epoch, removal.slot));

    let list = uniform_list("queue-drawer-list", count, move |range, _window, _cx| {
        range
            .map(|slot_index| {
                let slot = slots[slot_index];
                let element: AnyElement = match slot {
                    QueueSlot::History(count) => {
                        queue_section_row(&language.textf("已播放 · {} 首", &[count.to_string()]))
                    }
                    QueueSlot::NowPlaying => queue_section_row(language.text("正在播放")),
                    QueueSlot::Upcoming(count) => {
                        queue_section_row(&language.textf("接下来 · {} 首", &[count.to_string()]))
                    }
                    QueueSlot::Track { index, playing } => {
                        let Some(track) = tracks.get(index) else {
                            return div().h(px(QUEUE_ROW_H)).into_any_element();
                        };
                        let title = track.title.clone();
                        let artist = track.artist.clone();
                        let duration = track.duration_label();
                        let entity = entity.clone();
                        let hover_entity = entity.clone();
                        // 「正在播放」高亮：旧行 160ms 淡出、新行淡入，底色和标题一起变。
                        let strength = playing_strength(
                            np_current.as_deref(),
                            np_previous.as_deref(),
                            np_t,
                            &track.id,
                        );
                        let title_color =
                            motion::mix_rgba(theme::text(), theme::accent(), strength);
                        // 队列里的每一行都用同一份 96px 缩略图；正在播放的行在全尺寸
                        // 封面先返回时用它兜底，避免刚开抽屉时只有这一行有图。
                        let cover = track
                            .id
                            .strip_prefix(sodam_core::local::LOCAL_ID_PREFIX)
                            .and_then(|path| covers.get(path).cloned().flatten())
                            .or_else(|| playing.then(|| now_cover.clone()).flatten());
                        let row = div()
                            .id(("queue-slot", slot_index))
                            .relative()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(theme::space::MD))
                            .h(px(QUEUE_ROW_H))
                            .px(px(theme::space::XL))
                            .cursor_pointer()
                            .bg(motion::with_alpha(theme::accent(), 0.18 * strength))
                            .when_some(
                                motion::row_hover_layer(
                                    "queue-row",
                                    hover_state.fade_for(slot_index),
                                    0.0,
                                    theme::surface_hover(),
                                ),
                                |this, layer| this.child(layer),
                            )
                            .on_hover(move |hovered: &bool, _window, cx: &mut gpui::App| {
                                let next = if *hovered { Some(slot_index) } else { None };
                                hover_entity.update(cx, |root, cx| {
                                    let changed = match next {
                                        Some(_) => root.queue_hover.set(next),
                                        None => {
                                            root.queue_hover.current == Some(slot_index)
                                                && root.queue_hover.set(None)
                                        }
                                    };
                                    if changed {
                                        cx.notify();
                                    }
                                });
                            })
                            .on_click({
                                let entity = entity.clone();
                                move |_event: &ClickEvent, _window, cx: &mut gpui::App| {
                                    entity.update(cx, |root, cx| {
                                        root.close_queue_menu(cx);
                                        root.jump_in_queue(index, cx);
                                        cx.notify();
                                    });
                                }
                            })
                            .on_mouse_down(MouseButton::Right, {
                                let entity = entity.clone();
                                let track_id = track.id.clone();
                                move |event: &MouseDownEvent, _window, cx: &mut gpui::App| {
                                    let position = event.position;
                                    entity.update(cx, |root, cx| {
                                        let engine_id = root.engine.snapshot().track_id;
                                        if playing || track_id == engine_id {
                                            return;
                                        }
                                        root.open_queue_menu(index, position, cx);
                                    });
                                }
                            })
                            .child(match cover {
                                // 抽屉每次打开都会重新挂载行；这里用静态图，
                                // 不能复用列表里「首次出现淡入」的 thumb_img，
                                // 否则有歌时每次开抽屉封面都会闪一下。
                                Some(path) => div()
                                    .flex_none()
                                    .size(px(theme::size::ROW_ART))
                                    .rounded(px(4.0))
                                    .overflow_hidden()
                                    .child(img(path).size_full().object_fit(ObjectFit::Cover))
                                    .into_any_element(),
                                None => track_art(theme::size::ROW_ART, 4.0).into_any_element(),
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .truncate()
                                            .text_size(theme::Text::Body.size())
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(title_color)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(theme::Text::Small.size())
                                            .text_color(theme::text_muted())
                                            .child(artist),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(44.0))
                                    .text_right()
                                    .text_size(theme::Text::Small.size())
                                    .text_color(theme::text_faint())
                                    .child(duration),
                            );
                        row.into_any_element()
                    }
                };
                // 删除后被删行下面的所有槽位（包括分区标题）都从旧位置滑回新位置。
                match reflow {
                    Some((epoch, removed)) if slot_index > removed => div()
                        .w_full()
                        .h(px(QUEUE_ROW_H))
                        .child(element)
                        .with_animation(
                            gpui::ElementId::Name(
                                format!("queue-reflow-{epoch}-{slot_index}").into(),
                            ),
                            motion::standard(motion::duration::QUEUE_REFLOW),
                            |this, delta| this.top(px(QUEUE_ROW_H * (1.0 - delta))),
                        )
                        .into_any_element(),
                    _ => element,
                }
            })
            .collect::<Vec<_>>()
    })
    .flex_1()
    .min_h(px(0.0))
    .scrollbar_width(px(0.0))
    .track_scroll(&root.queue_scroll)
    .on_scroll_wheel(crate::app_scroll::uniform_wheel_listener(
        cx,
        "queue-drawer",
        root.queue_scroll.clone(),
    ));

    let menu_tracks = root.queue_cache.1.clone();
    let menu = root.queue_menu.as_ref().and_then(|menu_state| {
        let track = menu_tracks.get(menu_state.index)?.clone();
        Some((menu_state.index, menu_state.position, track))
    });
    let menu_closing = root.queue_menu_closing;
    let menu_seq = root.queue_menu_seq;
    let drawer_close_fade = root.hovers.get("drawer-close");
    let remove_fade = root.hovers.get("queue-remove-item");
    // 被删行的幽灵层：停在原槽位淡出并向左滑 6px，下面的行同时上移。
    let ghost = recent_removal.map(|removal| {
        let track = &removal.track;
        let cover = track
            .id
            .strip_prefix(sodam_core::local::LOCAL_ID_PREFIX)
            .and_then(|path| ghost_covers.get(path).cloned().flatten());
        // 滚动偏移 offset().y 向下为负，换成「已滚过多少」要相加。
        let top = removal.slot as f32 * QUEUE_ROW_H + removal.scroll_top;
        div()
            .absolute()
            .top(px(top))
            .left(px(0.0))
            .w_full()
            .h(px(QUEUE_ROW_H))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::space::MD))
            .px(px(theme::space::XL))
            .bg(theme::glass_panel())
            .with_animation(
                gpui::ElementId::Name(format!("queue-remove-{}", removal.epoch).into()),
                motion::standard(motion::duration::QUEUE_ROW_OUT),
                |this, delta| this.opacity(1.0 - delta).left(px(-6.0 * delta)),
            )
            .child(match cover {
                Some(path) => div()
                    .flex_none()
                    .size(px(theme::size::ROW_ART))
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .child(img(path).size_full().object_fit(ObjectFit::Cover))
                    .into_any_element(),
                None => track_art(theme::size::ROW_ART, 4.0).into_any_element(),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::Text::Body.size())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme::text())
                            .child(track.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::Text::Small.size())
                            .text_color(theme::text_muted())
                            .child(track.artist.clone()),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(44.0))
                    .text_right()
                    .text_size(theme::Text::Small.size())
                    .text_color(theme::text_faint())
                    .child(track.duration_label()),
            )
    });
    let list_area = div()
        .relative()
        .overflow_hidden()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .child(list)
        .when_some(ghost, |this, ghost| this.child(ghost));

    div()
        .relative()
        .flex()
        .flex_col()
        .size_full()
        .w(px(QUEUE_WIDTH))
        .flex_none()
        .bg(theme::glass_panel())
        // 真正的 backdrop blur：把下面的页面糊成磨砂玻璃（DirectX 路径支持）。
        .backdrop_blur(px(24.0))
        .border_l_1()
        .border_color(theme::glass_highlight())
        .shadow_lg()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(theme::space::SM))
                .h(px(theme::TOOLBAR_H))
                .flex_none()
                .px(px(theme::space::LG))
                .border_b_1()
                .border_color(theme::border())
                // 收起按钮放左边：抽屉盖住了窗口控制按钮，若也放右边，
                // 点击收起的同一位置会落到下面的「最大化」上（实测会误最大化）。
                // 图标是钝角 `›`，表示面板向右收起。
                .child(
                    icon_button(
                        "drawer-close",
                        "chevron-right-wide",
                        theme::ICON_SM,
                        theme::text_muted(),
                        drawer_close_fade,
                    )
                    .on_hover(Root::hover_listener(cx, "drawer-close"))
                    .on_click(cx.listener(
                        |root, _event: &ClickEvent, _window, cx| {
                            root.close_queue(cx);
                        },
                    )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(theme::Text::Body.size())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(language.textf("播放队列 · {} 首", &[total.to_string()])),
                ),
        )
        .child(list_area)
        .when_some(menu, |this, (index, position, track)| {
            let menu_el = div()
                .id("queue-menu")
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation();
                })
                .flex()
                .flex_col()
                .w(px(200.0))
                .p(px(theme::space::XS))
                .rounded(px(theme::radius::ROW))
                .bg(theme::glass_panel())
                .backdrop_blur(px(32.0))
                .border_1()
                .border_color(theme::border_strong())
                .shadow_lg()
                .child(
                    div()
                        .px(px(theme::space::MD))
                        .py(px(theme::space::XS))
                        .truncate()
                        .text_size(theme::Text::Tiny.size())
                        .text_color(theme::text_faint())
                        .child(track.title.clone()),
                )
                .child(
                    div()
                        .id("queue-remove")
                        .relative()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::space::SM))
                        .h(px(40.0))
                        .px(px(theme::space::MD))
                        .rounded(px(theme::radius::ROW))
                        .cursor_pointer()
                        .child(motion::hover_layer(
                            "queue-remove-item",
                            remove_fade,
                            theme::radius::ROW,
                            theme::surface_hover(),
                        ))
                        .on_hover(Root::hover_listener(cx, "queue-remove-item"))
                        .text_size(theme::Text::Body.size())
                        .text_color(theme::text())
                        .on_click(cx.listener(move |root, _event: &ClickEvent, _window, cx| {
                            root.remove_from_queue(index, cx);
                        }))
                        .child(
                            svg()
                                .path(icons::path("x"))
                                .size(px(theme::ICON_SM))
                                .text_color(theme::text_muted()),
                        )
                        .child(root.tr("从队列移除")),
                );
            // 菜单 160ms 从点击位置放大淡入；关闭时 90ms 回缩淡出。
            let menu_el = if menu_closing {
                menu_el
                    .with_animation(
                        gpui::ElementId::Name(format!("queue-menu-out-{menu_seq}").into()),
                        motion::standard(motion::duration::MENU_OUT),
                        |this, delta| this.opacity(1.0 - delta).layer_scale(1.0 - 0.02 * delta),
                    )
                    .into_any_element()
            } else {
                menu_el
                    .with_animation(
                        gpui::ElementId::Name(format!("queue-menu-in-{menu_seq}").into()),
                        motion::standard(motion::duration::MENU_IN),
                        |this, delta| this.opacity(delta).layer_scale(0.96 + 0.04 * delta),
                    )
                    .into_any_element()
            };
            this.child(
                gpui::deferred(
                    gpui::anchored()
                        .snap_to_window()
                        .position(gpui::point(position.x, position.y))
                        .child(menu_el),
                )
                .with_priority(2),
            )
        })
}
