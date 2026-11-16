//! 「正在播放」页：左侧大封面 + 曲目信息，右侧同步歌词。
//!
//! 版式对齐汽水音乐播放页：内容整体居中，封面 360、间距 56、歌词面板 560。
//! 封面来自内嵌图片，歌词来自内嵌标签或同名 .lrc；两者都在后台线程准备。
//!
//! 歌词不是静态列表：当前行放大加粗、上下行随距离缩小；行内按字符/单词
//! 从左到右染色。英文按「当前行字号」预计算平衡断句，行高随内容和译文行数
//! 变化；滚动锚点固定在当前英文文本块的中心，译文显隐不会带动英文位置。

use std::ops::Range;

use gpui::prelude::*;
use gpui::{div, px, svg, AnimationExt, AnyElement, App, Context, IntoElement, Window};

use crate::app::{line_progress, lyric_tokens, LyricToken, Root};
use crate::ui::{icons, motion, theme};
use sodam_core::lyrics::Lyrics;

/// 没有排版缓存时的回退行高（首帧/旧数据），实际行高由 [`LyricsLayout`] 给出。
pub(crate) const LYRIC_ROW_H: f32 = 64.0;
/// 播放页左侧封面与歌词之间的间距。
const PLAYER_GAP: f32 = 56.0;
/// 歌词面板的最大宽度。
const LYRIC_PANEL_W: f32 = 560.0;
/// 主歌词字号：普通行 / 当前行。
///
/// 换行断点永远按当前行字号（较大的那个）测量，放大时不会重新断行。
/// 歌词字体与行高规格：由设置的「字体大小」和「字体粗细」派生。
#[derive(Clone, Copy, PartialEq, Eq)]
struct LyricTypography {
    size: u16,
    weight: u16,
}

impl LyricTypography {
    fn from_settings(settings: &sodam_core::config::Settings) -> Self {
        Self {
            size: settings.lyrics_font_size,
            weight: settings.lyrics_font_weight,
        }
    }

    fn base_size(self) -> f32 {
        self.active_size() * 0.77
    }

    fn active_size(self) -> f32 {
        f32::from(self.size)
    }

    fn english_line_h(self) -> f32 {
        (self.active_size() * 1.3).ceil()
    }

    fn translation_size(self) -> f32 {
        (self.active_size() * 0.54).round().max(10.0)
    }

    fn translation_line_h(self) -> f32 {
        (self.translation_size() * 1.35).ceil()
    }

    fn weight(self) -> gpui::FontWeight {
        gpui::FontWeight(f32::from(self.weight))
    }
}

const LYRIC_BLOCK_GAP: f32 = 5.0;
const LYRIC_ROW_PAD: f32 = 12.0;
/// 译文显隐时向下/向上的位移。
const TRANSLATION_SHIFT: f32 = 4.0;
pub(crate) fn player_view(root: &mut Root, window: &Window, cx: &mut Context<Root>) -> AnyElement {
    let track = root
        .pending_track
        .clone()
        .or_else(|| root.queue.current().cloned());
    let Some(track) = track else {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(theme::Text::Body.size())
            .text_color(theme::text_muted())
            .child(root.tr("还没有开始播放"))
            .with_animation(
                "player-empty",
                motion::standard(motion::duration::STATE),
                |this, delta| this.opacity(delta),
            )
            .into_any_element();
    };

    // 播放页与播放栏共用同一份文字交换状态：两边同时开始交叉淡化。
    root.note_track_text(&track.title, &track.artist, &track.album);
    let text_swap = root
        .track_text_swap
        .as_ref()
        .filter(|swap| swap.active())
        .map(|swap| {
            (
                swap.from_title.clone(),
                swap.from_artist.clone(),
                swap.from_album.clone(),
                swap.seq,
                swap.direction,
            )
        });
    // 大封面双层交叉淡化：旧封面保持显示，新封面 280ms 淡入 + 0.985→1 放大。
    let cover = root.now_playing_cover();
    root.page_cover_fade.update(cover);
    let cover_placeholder = div()
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
                .size(px(72.0))
                .text_color(theme::text_faint()),
        )
        .into_any_element();
    let mut cover_element = div()
        .relative()
        .size(px(theme::HERO_COVER))
        .rounded(px(theme::radius::ART))
        .overflow_hidden()
        .shadow_lg();
    for layer in motion::cover_layers(
        "page-hero",
        &root.page_cover_fade,
        motion::duration::COVER,
        theme::radius::ART,
        cover_placeholder,
    ) {
        cover_element = cover_element.child(layer);
    }
    let cover_block = div()
        .flex()
        .flex_col()
        .items_start()
        .gap(px(theme::space::XL))
        .flex_none()
        .child(cover_element)
        .child({
            let title_style = |text: String| -> AnyElement {
                div()
                    .w_full()
                    .truncate()
                    .text_size(px(26.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::text())
                    .child(text)
                    .into_any_element()
            };
            let artist_style = |text: String| -> AnyElement {
                div()
                    .w_full()
                    .truncate()
                    .text_size(theme::Text::Body.size())
                    .text_color(theme::text_muted())
                    .child(text)
                    .into_any_element()
            };
            let album_style = |text: String| -> AnyElement {
                div()
                    .w_full()
                    .truncate()
                    .text_size(theme::Text::Small.size())
                    .text_color(theme::text_faint())
                    .child(text)
                    .into_any_element()
            };
            let new_title = title_style(track.title.clone());
            let new_artist = artist_style(track.artist.clone());
            let new_album = album_style(track.album.clone());
            let mut info = div()
                .w_full()
                .max_w(px(theme::HERO_COVER))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(theme::space::SM));
            info = info.child(match &text_swap {
                Some((from, _, _, seq, direction)) => motion::crossfade_text(
                    "page-title",
                    *seq,
                    Some(title_style(from.clone())),
                    new_title,
                    motion::duration::CONTENT,
                    4.0 * direction,
                ),
                None => new_title,
            });
            info = info.child(match &text_swap {
                Some((_, from, _, seq, direction)) => motion::crossfade_text(
                    "page-artist",
                    *seq,
                    Some(artist_style(from.clone())),
                    new_artist,
                    motion::duration::CONTENT,
                    4.0 * direction,
                ),
                None => new_artist,
            });
            if !track.album.trim().is_empty() {
                info = info.child(match &text_swap {
                    Some((_, _, from, seq, direction)) if !from.trim().is_empty() => {
                        motion::crossfade_text(
                            "page-album",
                            *seq,
                            Some(album_style(from.clone())),
                            new_album,
                            motion::duration::CONTENT,
                            4.0 * direction,
                        )
                    }
                    _ => new_album,
                });
            }
            info
        });

    let lyrics = lyrics_panel(root, window, cx);

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(PLAYER_GAP))
        .size_full()
        .px(px(theme::space::XXL))
        .child(cover_block)
        .child(lyrics)
        .into_any_element()
}

/// 歌词面板宽度：与 `player_view` 的布局算式一致，换行测量使用同一宽度。
fn lyrics_panel_width(window: &Window) -> f32 {
    let viewport = f32::from(window.viewport_size().width);
    (viewport - theme::SIDEBAR_W - theme::space::XXL * 2.0 - theme::HERO_COVER - PLAYER_GAP)
        .clamp(280.0, LYRIC_PANEL_W)
}

struct LyricTextLine {
    text: gpui::SharedString,
    width: f32,
    tokens: Vec<LyricToken>,
}

/// 一行歌词的预计算排版。
struct LyricsRow {
    /// 主歌词按平衡断句拆成若干行，保留整行整形宽度用于扫色。
    english: Vec<LyricTextLine>,
    /// 译文按面板宽度拆行；没有译文时是空串占位（高度仍然保留）。
    translation: Vec<String>,
    /// 整行高度（含上下内边距）。
    height: f32,
    /// 英文文本块高度。
    english_h: f32,
    /// 译文块高度；没有译文时为 0。
    translation_h: f32,
    /// 行首相对歌词内容顶部的累计偏移。
    offset: f32,
}

/// 整首歌的歌词排版缓存。滚动锚点、行高、断句都从这里读。
pub(crate) struct LyricsLayout {
    key: (u64, u32, u16, u16),
    rows: Vec<LyricsRow>,
    has_translation: bool,
}

impl LyricsLayout {
    fn build(
        lyrics: &Lyrics,
        width: f32,
        typo: LyricTypography,
        window: &Window,
        key: (u64, u32, u16, u16),
    ) -> Self {
        let available = (width - theme::space::SM * 2.0).max(120.0);
        let has_translation = lyrics.lines.iter().any(|line| {
            line.translation
                .as_deref()
                .is_some_and(|translation| !translation.trim().is_empty())
        });
        let style = window.text_style();
        let system = window.text_system();
        let measure = |text: &str, size: f32, weight: gpui::FontWeight| -> f32 {
            let mut run = style.to_run(text.len());
            run.font.weight = weight;
            f32::from(system.layout_line(text, px(size), &[run], None).width)
        };

        let active_size = typo.active_size();
        let active_weight = typo.weight();
        let trans_size = typo.translation_size();
        let english_line_h = typo.english_line_h();
        let trans_line_h = typo.translation_line_h();

        let mut rows = Vec::with_capacity(lyrics.lines.len());
        let mut offset = 0.0;
        for line in &lyrics.lines {
            let english_tokens = split_overlong(lyric_tokens(&line.text), available, |text| {
                measure(text, active_size, active_weight)
            });
            let english_ranges = wrap_tokens(&english_tokens, available, &|text| {
                measure(text, active_size, active_weight)
            });
            let english: Vec<_> = english_ranges
                .into_iter()
                .map(|range| {
                    let tokens = english_tokens[range].to_vec();
                    let text: String = tokens.iter().map(|token| token.text.as_str()).collect();
                    LyricTextLine {
                        width: measure(&text, active_size, active_weight),
                        text: text.into(),
                        tokens,
                    }
                })
                .collect();

            let translation: Vec<String> = if has_translation {
                let raw = line.translation.clone().unwrap_or_default();
                let tokens = split_overlong(lyric_tokens(&raw), available, |text| {
                    measure(text, trans_size, gpui::FontWeight::NORMAL)
                });
                wrap_tokens(&tokens, available, &|text| {
                    measure(text, trans_size, gpui::FontWeight::NORMAL)
                })
                .into_iter()
                .map(|range| {
                    tokens[range]
                        .iter()
                        .map(|token| token.text.as_str())
                        .collect()
                })
                .collect()
            } else {
                Vec::new()
            };

            let english_h = english.len().max(1) as f32 * english_line_h;
            let translation_h = if has_translation {
                translation.len().max(1) as f32 * trans_line_h
            } else {
                0.0
            };
            let height = LYRIC_ROW_PAD * 2.0
                + english_h
                + if has_translation {
                    LYRIC_BLOCK_GAP + translation_h
                } else {
                    0.0
                };
            rows.push(LyricsRow {
                english,
                translation,
                height,
                english_h,
                translation_h,
                offset,
            });
            offset += height;
        }

        Self {
            key,
            rows,
            has_translation,
        }
    }

    /// 当前英文文本块中心相对歌词内容顶部的偏移。
    pub(crate) fn anchor_offset(&self, index: usize) -> Option<f32> {
        self.rows
            .get(index)
            .map(|row| row.offset + LYRIC_ROW_PAD + row.english_h * 0.5)
    }
}

/// 歌词或面板宽度/字体设置变化时重建排版；同一份配置只算一次。
fn ensure_lyrics_layout(
    root: &mut Root,
    lyrics: &Lyrics,
    seq: u64,
    width: f32,
    typo: LyricTypography,
    window: &Window,
) {
    let key = (seq, (width * 2.0).round() as u32, typo.size, typo.weight);
    if root
        .lyrics_layout
        .as_ref()
        .is_some_and(|layout| layout.key == key)
    {
        return;
    }
    root.lyrics_layout = Some(LyricsLayout::build(lyrics, width, typo, window, key));
}

fn lyrics_panel(root: &mut Root, window: &Window, cx: &mut Context<Root>) -> AnyElement {
    let lyrics_seq = root.lyrics_seq;
    let Some(lyrics) = root.lyrics.clone() else {
        return div()
            .w(px(lyrics_panel_width(window)))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(theme::space::MD))
            .child(
                svg()
                    .path(icons::path("list-music"))
                    .size(px(48.0))
                    .text_color(theme::text_faint()),
            )
            .child(
                div()
                    .text_size(theme::Text::Body.size())
                    .text_color(theme::text_muted())
                    .child(root.tr("这首歌没有内嵌歌词")),
            )
            .child(
                div()
                    .text_size(theme::Text::Small.size())
                    .text_color(theme::text_faint())
                    .child(root.tr("支持内嵌 LRC 歌词或同名 .lrc 文件")),
            )
            // 切到一首没有歌词的歌时，空态与歌词块使用同一节奏淡入。
            .with_animation(
                gpui::ElementId::Name(format!("lyrics-empty-{lyrics_seq}").into()),
                motion::standard(motion::duration::LYRIC_LOAD),
                |this, delta| this.opacity(0.15 + 0.85 * delta),
            )
            .into_any_element();
    };

    let panel_w = lyrics_panel_width(window);
    let show_translation = root.settings.lyrics_translation;
    let toggle_seq = root.lyrics_translation_seq;
    let toggle_target = f32::from(show_translation as u8);
    // 动画窗口结束后把起点对齐终点：页面重开/重绘不会重播。
    if toggle_seq > 0
        && root
            .lyrics_translation_at
            .is_some_and(|at| at.elapsed() >= motion::duration::LYRIC_TRANSLATION_TOGGLE)
    {
        root.lyrics_translation_from = toggle_target;
    }
    let toggle_from = if toggle_seq == 0 {
        toggle_target
    } else {
        root.lyrics_translation_from
    };
    let typo = LyricTypography::from_settings(&root.settings);
    let lyrics_blur = root.settings.lyrics_blur;
    let lyrics_staggered = root.settings.lyrics_staggered_scroll;
    let lyrics_3d = root.settings.lyrics_3d_effect;
    let continuous_sweep = root.settings.lyrics_continuous_sweep;
    ensure_lyrics_layout(root, &lyrics, lyrics_seq, panel_w, typo, window);
    let layout = root.lyrics_layout.as_ref().expect("刚刚建好");
    let has_translation = layout.has_translation;

    let synced = lyrics.synced;
    let active = root.lyrics_active;
    let prev_active = root.lyrics_prev_active;
    let anim_seq = root.lyrics_anim_seq;
    let translation_t = root
        .lyrics_switch_at
        .map(|at| motion::progress(at, motion::duration::LYRIC_TRANSLATION))
        .unwrap_or(1.0);
    let out_t = root
        .lyrics_switch_at
        .map(|at| motion::progress(at, motion::duration::LYRIC_OUT))
        .unwrap_or(1.0);
    // 旧行只在换行后的退场窗口里保留「已唱完」样式；超时后切回普通样式，
    // 否则上一行会一直保持白色加粗，和上下文的层级对不上。
    let switch_recent = root
        .lyrics_switch_at
        .is_some_and(|at| at.elapsed() < motion::duration::LYRIC_OUT);
    let position_ms = (root.display_position_seconds().max(0.0) * 1000.0).round() as u64;
    let pending_color = theme::lyrics_pending();
    let inactive_color = theme::lyrics_inactive();
    let sung_color = theme::lyrics_sung();
    let entity = cx.entity();
    let hover_state = root.lyrics_hover;
    let scroll_handle = root.lyrics_scroll.clone();
    let now = std::time::Instant::now();
    let reading = root.lyrics_hold_until.is_some_and(|until| now < until);
    let scroll_top = crate::ui::smooth_scroll::read(&scroll_handle);
    let stagger = lyrics_staggered
        .then_some(root.lyrics_scroll_stagger)
        .flatten()
        .map(|(at, start)| {
            (
                now.saturating_duration_since(at).as_secs_f32(),
                scroll_top - start,
            )
        });

    // 上下留白让首行/末行也能滚到正中。用滚动句柄的实际视口高度；
    // 第一帧还没布局时用窗口高度减去顶栏和播放栏估算。
    let viewport_h = f32::from(scroll_handle.bounds().size.height);
    let viewport_h = if viewport_h > 0.0 {
        viewport_h
    } else {
        (f32::from(window.viewport_size().height) - theme::TOOLBAR_H - theme::PLAYER_H).max(0.0)
    };
    // 锚点是英文文本块中心：内容顶部留半个视口，目标偏移就是 `offsets + 英文块中心`。
    let pad = viewport_h * 0.5;

    let rows = layout
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let is_active = synced && active == Some(index);
            let was_active = synced && prev_active == Some(index) && switch_recent;
            let distance = active.map(|active| active.abs_diff(index));
            let depth = distance.unwrap_or(0).min(5) as f32;
            let previous_depth =
                prev_active.map_or(depth, |prev| prev.abs_diff(index).min(5) as f32);
            let depth =
                previous_depth + (depth - previous_depth) * motion::ease_in_out_cubic(out_t);
            let row_top = pad + row.offset - scroll_top;
            let visible = row_top + row.height > 0.0 && row_top < viewport_h;
            let shift = match (stagger, active, distance) {
                (Some((elapsed, scroll_delta)), Some(active), Some(distance @ 1..=6)) => {
                    let lag = if index > active { 1.0 } else { 0.65 };
                    let max_shift = row.height * 0.38;
                    stagger_offset(
                        elapsed,
                        scroll_delta.clamp(-max_shift, max_shift) * lag,
                        distance,
                    )
                }
                _ => 0.0,
            };
            let center = row_top + row.height * 0.5 + shift - pad;
            let (scale, inset, curve) = if lyrics_3d {
                lyric_perspective(center, pad)
            } else {
                (1.0, 0.0, 0.0)
            };

            // 刚唱完的行保留整行点亮，
            // 跟着退场动画缩小，并渐变到普通歌词颜色。
            let english = if is_active {
                let wipe = line_progress(&lyrics, index, position_ms);
                let render = if continuous_sweep {
                    english_wipe
                } else {
                    english_word_highlight
                };
                render(
                    &row.english,
                    wipe,
                    pending_color,
                    sung_color,
                    typo.english_line_h(),
                )
            } else if was_active {
                english_plain(
                    &row.english,
                    motion::mix_rgba(sung_color, inactive_color, out_t),
                    typo.english_line_h(),
                )
            } else {
                english_plain(&row.english, inactive_color, typo.english_line_h())
            };

            // 主歌词单独包在固定高度的区域里：高亮只缩放文字本身，
            // 译文留在缩放层之外，不会在换行后退场时突然变大。
            let main = div()
                .w_full()
                .h(px(row.english_h))
                .flex()
                .flex_col()
                .justify_center()
                .text_size(px(if is_active || was_active {
                    typo.active_size()
                } else {
                    typo.base_size()
                }))
                .font_weight(typo.weight())
                .child(english);
            let main = if is_active {
                // 字号切换用图层缩放而不是逐帧改 font-size：文字只按高亮字号
                // 光栅化一次，之后由 GPU 连续缩放，换行时不会因为重新排字卡一下。
                let scale_from = typo.base_size() / typo.active_size();
                main.with_animation(
                    gpui::ElementId::Name(format!("lyric-grow-{anim_seq}-{index}").into()),
                    motion::standard(motion::duration::LYRIC),
                    move |this, delta| {
                        this.layer_scale_origin(gpui::point(0.0, 0.5))
                            .layer_scale(scale_from + (1.0 - scale_from) * delta)
                    },
                )
                .into_any_element()
            } else if was_active {
                let scale_to = typo.base_size() / typo.active_size();
                main.with_animation(
                    gpui::ElementId::Name(format!("lyric-shrink-{anim_seq}-{index}").into()),
                    motion::standard(motion::duration::LYRIC_OUT),
                    move |this, delta| {
                        this.layer_scale_origin(gpui::point(0.0, 0.5))
                            .layer_scale(1.0 + (scale_to - 1.0) * delta)
                    },
                )
                .into_any_element()
            } else {
                main.into_any_element()
            };

            // 译文固定占英文块下方：始终保留占位，开关只改透明度和 4px 位移，
            // 英文位置与整块滚动都不动。
            let translation = if has_translation {
                let color = translation_color(is_active, was_active, distance, translation_t);
                let mut block = div()
                    .w_full()
                    .h(px(row.translation_h))
                    .flex()
                    .flex_col()
                    .justify_center()
                    .text_size(px(typo.translation_size()))
                    .font_weight(gpui::FontWeight::NORMAL);
                for text in &row.translation {
                    block = block.child(
                        div()
                            .w_full()
                            .h(px(typo.translation_line_h()))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .text_color(color)
                            .child(text.clone()),
                    );
                }
                let node = if toggle_seq == 0 {
                    block.opacity(toggle_target).into_any_element()
                } else {
                    block
                        .with_animation(
                            gpui::ElementId::Name(
                                format!("lyric-translation-{toggle_seq}-{index}").into(),
                            ),
                            motion::soft(motion::duration::LYRIC_TRANSLATION_TOGGLE),
                            move |this, delta| {
                                let top_from = (1.0 - toggle_from) * TRANSLATION_SHIFT;
                                let top_to = if toggle_target > 0.5 {
                                    0.0
                                } else {
                                    TRANSLATION_SHIFT
                                };
                                this.opacity(toggle_from + (toggle_target - toggle_from) * delta)
                                    .top(px(top_from + (top_to - top_from) * delta))
                            },
                        )
                        .into_any_element()
                };
                Some(node)
            } else {
                None
            };

            let mut content = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(LYRIC_BLOCK_GAP))
                .child(main);
            if let Some(translation) = translation {
                content = content.child(translation);
            }

            let fade = match distance {
                None | Some(0) => 1.0,
                _ if reading || hover_state.current == Some(index) => 0.96,
                Some(1) => 0.94,
                Some(2) => 0.78,
                _ => 0.62,
            };
            if lyrics_blur
                && visible
                && !reading
                && !is_active
                && hover_state.current != Some(index)
                && depth > 0.25
            {
                let blur_px = ((depth - 0.35).max(0.0) * 1.05 + 0.35).min(4.2);
                content = content.blur(px(blur_px));
            }
            let (effective_scale, effective_inset) = if lyrics_3d && visible {
                (scale, inset)
            } else {
                (1.0, 0.0)
            };
            let content = if lyrics_3d && visible {
                content
                    .pl(px(inset))
                    .layer_scale_origin(gpui::point(0.0, 0.5))
                    .layer_scale(scale)
            } else {
                content
            };
            let content = if is_active {
                content
                    .with_animation(
                        gpui::ElementId::Name(format!("lyric-fade-in-{anim_seq}-{index}").into()),
                        motion::standard(motion::duration::LYRIC),
                        |this, delta| this.opacity(0.82 + 0.18 * delta),
                    )
                    .into_any_element()
            } else if was_active {
                // 退场结束时的亮度必须等于它作为普通行的目标亮度，否则 380ms 后
                // 会从 0.78 突跳到 1.0（相邻行）——看起来像亮了一下。
                content
                    .with_animation(
                        gpui::ElementId::Name(format!("lyric-fade-out-{anim_seq}-{index}").into()),
                        motion::standard(motion::duration::LYRIC_OUT),
                        move |this, delta| this.opacity(1.0 - (1.0 - fade) * delta),
                    )
                    .into_any_element()
            } else {
                content.opacity(fade).into_any_element()
            };

            let active_weight = if is_active {
                1.0
            } else if was_active {
                1.0 - motion::ease_in_out_cubic(out_t)
            } else {
                0.0
            };
            let content_h = (row.height - LYRIC_ROW_PAD * 2.0).max(1.0);
            let hover_frame = lyric_hover_frame(
                panel_w,
                content_h,
                row.english_h,
                active_weight,
                effective_scale,
                effective_inset,
            );
            let at_ms = lyrics.lines.get(index).and_then(|line| line.at_ms);
            let hover_entity = entity.clone();
            let click_entity = entity.clone();
            let hover_pill = at_ms.and_then(|_| {
                let layer = motion::row_hover_layer(
                    "lyric-row",
                    hover_state.fade_for(index),
                    (theme::radius::ROW * effective_scale).max(6.0),
                    theme::lyrics_hover_bg(is_active),
                )?;
                Some(
                    div()
                        .absolute()
                        .left(px(hover_frame.left))
                        .top(px(hover_frame.top))
                        .w(px(hover_frame.width))
                        .h(px(hover_frame.height))
                        .child(layer),
                )
            });

            div()
                .id(("lyric-line", index))
                .relative()
                .w_full()
                .h(px(row.height))
                .flex()
                .flex_col()
                .justify_center()
                .when(at_ms.is_some(), |this| {
                    this.cursor_pointer()
                        .on_hover(move |hovered: &bool, _window, cx: &mut App| {
                            let next = if *hovered { Some(index) } else { None };
                            hover_entity.update(cx, |root, cx| {
                                let changed = match next {
                                    Some(_) => root.lyrics_hover.set(next),
                                    None => {
                                        root.lyrics_hover.current == Some(index)
                                            && root.lyrics_hover.set(None)
                                    }
                                };
                                if changed {
                                    cx.notify();
                                }
                            });
                        })
                        .on_click(move |_event: &gpui::ClickEvent, _window, cx: &mut App| {
                            let Some(at_ms) = at_ms else { return };
                            click_entity.update(cx, |root, cx| {
                                root.seek_to_ms(at_ms);
                                root.lyrics_scroll_line = None;
                                root.sync_lyrics_position(cx);
                                cx.notify();
                            });
                        })
                })
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(content_h))
                        .top(px(shift + curve))
                        .when_some(hover_pill, |this, pill| this.child(pill))
                        .child(
                            div()
                                .w_full()
                                .h_full()
                                .px(px(theme::space::SM))
                                .child(content),
                        ),
                )
                .into_any_element()
        })
        .collect::<Vec<_>>();

    let mut panel = div()
        .w(px(panel_w))
        .min_w(px(280.0))
        .flex_shrink(1.0)
        .h_full()
        .flex()
        .flex_col();
    panel = panel.child(
        crate::app_scroll::scroll_area("lyrics-scroll", cx, scroll_handle)
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .w_full()
            .fade_edges(px(48.0), px(64.0))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .pt(px(pad))
                    .pb(px(pad))
                    .children(rows),
            ),
    );
    panel
        // 换歌（歌词重新加载）时整块淡入，避免上一首的歌词瞬间换成下一首。
        .with_animation(
            gpui::ElementId::Name(format!("lyrics-panel-{lyrics_seq}").into()),
            motion::standard(motion::duration::LYRIC_LOAD),
            |this, delta| this.opacity(0.15 + 0.85 * delta),
        )
        .into_any_element()
}

/// 译文颜色：当前行柔和变亮，离开的行同步变暗，远处的行更暗。
fn translation_color(
    is_active: bool,
    was_active: bool,
    distance: Option<usize>,
    t: f32,
) -> gpui::Rgba {
    let dim = theme::lyrics_translation();
    let bright = theme::lyrics_translation_active();
    if is_active {
        motion::mix_rgba(dim, bright, t)
    } else if was_active {
        motion::mix_rgba(bright, dim, t)
    } else if distance.is_some_and(|distance| distance <= 1) {
        dim
    } else {
        theme::lyrics_translation_faint()
    }
}

/// 普通歌词行：预先拆好的多行文本，每行整段排版（保留字距与标点），不换行、不截断。
fn english_plain(lines: &[LyricTextLine], color: gpui::Rgba, line_h: f32) -> AnyElement {
    let mut block = div().w_full().flex().flex_col();
    for line in lines {
        block = block.child(
            div()
                .w_full()
                .h(px(line_h))
                .flex()
                .items_center()
                .overflow_hidden()
                .child(div().flex_none().text_color(color).child(line.text.clone())),
        );
    }
    block.into_any_element()
}

fn english_word_highlight(
    lines: &[LyricTextLine],
    progress: f32,
    pending_color: gpui::Rgba,
    sung_color: gpui::Rgba,
    line_h: f32,
) -> AnyElement {
    let total = lines
        .iter()
        .flat_map(|line| &line.tokens)
        .map(|token| token.weight)
        .sum::<f32>();
    let mut remaining = total * progress;
    let highlight = gpui::HighlightStyle {
        color: Some(sung_color.into()),
        ..Default::default()
    };
    let mut block = div().w_full().flex().flex_col();
    for line in lines {
        let sung = sung_prefix_bytes(&line.tokens, &mut remaining);
        let text = gpui::StyledText::new(line.text.clone())
            .with_highlights((sung > 0).then_some((0..sung, highlight)));
        block = block.child(
            div()
                .w_full()
                .h(px(line_h))
                .flex()
                .items_center()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_color(pending_color)
                        .child(text),
                ),
        );
    }
    block.into_any_element()
}

fn sung_prefix_bytes(tokens: &[LyricToken], remaining: &mut f32) -> usize {
    let mut bytes = 0;
    for token in tokens {
        if *remaining < token.weight * 0.5 {
            *remaining = 0.0;
            break;
        }
        *remaining -= token.weight;
        bytes += token.text.len();
    }
    bytes
}

/// LRC 只有行时间戳。用整形后的宽度推进扫色，保留字距和连字。
fn english_wipe(
    lines: &[LyricTextLine],
    wipe: f32,
    pending_color: gpui::Rgba,
    sung_color: gpui::Rgba,
    line_h: f32,
) -> AnyElement {
    let mut remaining = lines.iter().map(|line| line.width).sum::<f32>() * wipe;
    let mut block = div().w_full().flex().flex_col();
    for line in lines {
        let sung_width = remaining.clamp(0.0, line.width);
        remaining -= line.width;
        let complete = sung_width >= line.width;
        let text = |color| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_color(color)
                .child(line.text.clone())
        };
        let mut row = div()
            .relative()
            .w_full()
            .h(px(line_h))
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .child(text(if complete { sung_color } else { pending_color }));
        if !complete && sung_width > 0.0 {
            row = row.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w(px(sung_width))
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .fade_sides(px(0.0), px((line.width - sung_width).min(5.0)))
                    .child(text(sung_color)),
            );
        }
        block = block.child(row);
    }
    block.into_any_element()
}

/// 用列表的真实滚动量补偿尚未开始移动的行，随后逐行归位。
fn stagger_offset(elapsed: f32, scroll_delta: f32, distance: usize) -> f32 {
    let delay = 0.095 * (1.0 - 0.75_f32.powi(distance as i32));
    let t = ((elapsed - delay) / 0.30).clamp(0.0, 1.0);
    scroll_delta * (1.0 - t * t * (3.0 - 2.0 * t))
}

/// 非当前行相对当前行的悬停底框宽度比例（不开立体时与 0.77 字号比例匹配）。
const INACTIVE_HOVER_WIDTH_RATIO: f32 = 0.80;

#[derive(Clone, Copy, Debug, PartialEq)]
struct LyricHoverFrame {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

/// 计算单行歌词「虚选中」背景框的几何尺寸，随当前行字号比例、立体缩进与景深缩放同步，
/// 保证悬停高亮严格贴合变换后的歌词块而不发生上下/左右错位。
fn lyric_hover_frame(
    panel_w: f32,
    content_h: f32,
    english_h: f32,
    active_weight: f32,
    scale: f32,
    inset: f32,
) -> LyricHoverFrame {
    const PILL_PAD_Y: f32 = 7.0;
    let active_weight = active_weight.clamp(0.0, 1.0);
    let scale = scale.max(0.1);
    let width_ratio =
        INACTIVE_HOVER_WIDTH_RATIO + (1.0 - INACTIVE_HOVER_WIDTH_RATIO) * active_weight;
    let left = (inset * scale).max(0.0);
    let max_width = (panel_w - left).max(0.0);
    let width = (panel_w * width_ratio * scale)
        .min(max_width)
        .max(120.0_f32.min(max_width));

    let origin_y = content_h * 0.5;
    let top_trim = english_h * 0.08 * (1.0 - active_weight);
    let raw_top = -PILL_PAD_Y + top_trim;
    let raw_bottom = content_h + PILL_PAD_Y;
    let top = origin_y + (raw_top - origin_y) * scale;
    let bottom = origin_y + (raw_bottom - origin_y) * scale;
    let height = (bottom - top).max(20.0);

    LyricHoverFrame {
        left,
        top,
        width,
        height,
    }
}

/// 按行在视口中的实际位置投影，滚动过程中深度随位置连续变化。
fn lyric_perspective(center: f32, half_height: f32) -> (f32, f32, f32) {
    let distance = (center.abs() / half_height.max(1.0)).min(1.0);
    let scale = 1.0 / (1.0 + 0.42 * distance.powf(1.3));
    let depth = 1.0 - scale;
    (
        scale,
        depth * 124.0,
        -center.clamp(-half_height, half_height) * depth * 0.30,
    )
}

/// 一行歌词的平衡式断句：先测量每个 token，再在合法单词边界上做动态规划。
///
/// 两端都慢的平衡不是目标而是手段：两行宽度尽量接近，同时在逗号/连词等
/// 自然停顿处断句；只有实在放不下才增加行数。
fn wrap_tokens(
    tokens: &[LyricToken],
    max_width: f32,
    measure: &dyn Fn(&str) -> f32,
) -> Vec<Range<usize>> {
    let count = tokens.len();
    if count == 0 {
        return std::iter::once(0..0).collect();
    }
    let widths: Vec<f32> = tokens
        .iter()
        .map(|token| measure(token.text.as_str()))
        .collect();
    let total: f32 = widths.iter().sum();
    if total <= max_width {
        return std::iter::once(0..count).collect();
    }

    // 贪心得到最少行数；行数定了再平衡每一行的宽度，避免把一句拆成很多碎行。
    let mut line_count = 1usize;
    let mut line_width = 0.0;
    for width in &widths {
        if line_width > 0.0 && line_width + width > max_width {
            line_count += 1;
            line_width = 0.0;
        }
        line_width += width;
    }

    let mut prefix = vec![0.0f32; count + 1];
    for (index, width) in widths.iter().enumerate() {
        prefix[index + 1] = prefix[index] + width;
    }
    let target = total / line_count as f32;
    let mut cost = vec![vec![f32::INFINITY; count + 1]; line_count + 1];
    let mut previous = vec![vec![usize::MAX; count + 1]; line_count + 1];
    cost[0][0] = 0.0;
    for line in 1..=line_count {
        for end in line..=count {
            for start in (line - 1)..end {
                if !cost[line - 1][start].is_finite() {
                    continue;
                }
                let width = prefix[end] - prefix[start];
                let overflow = (width - max_width).max(0.0);
                // 平衡项占主导；溢出只作为兜底，正常断句不会触发。
                let mut score = ((width - target) / max_width).powi(2) * 6.0;
                score += overflow * 1000.0;
                if start > 0 {
                    score += break_penalty(tokens, start);
                }
                if line == line_count && end == count && count > 1 {
                    let ratio = width / target.max(1.0);
                    if ratio < 0.55 {
                        score += (0.55 - ratio) * 10.0;
                    }
                }
                let next = cost[line - 1][start] + score;
                if next < cost[line][end] {
                    cost[line][end] = next;
                    previous[line][end] = start;
                }
            }
        }
    }

    if !cost[line_count][count].is_finite() {
        return greedy_ranges(&widths, max_width);
    }

    let mut ranges = Vec::with_capacity(line_count);
    let mut end = count;
    for line in (1..=line_count).rev() {
        let start = previous[line][end];
        if start == usize::MAX {
            return greedy_ranges(&widths, max_width);
        }
        ranges.push(start..end);
        end = start;
    }
    ranges.reverse();
    ranges
}

/// 单个 token 比整行还宽（长 URL、没有空格的超长单词）时，按 Unicode 字符拆开。
fn split_overlong(
    tokens: Vec<LyricToken>,
    max_width: f32,
    measure: impl Fn(&str) -> f32,
) -> Vec<LyricToken> {
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        if measure(token.text.as_str()) <= max_width {
            out.push(token);
            continue;
        }
        for ch in token.text.chars() {
            out.push(LyricToken {
                text: ch.to_string(),
                weight: 1.0,
            });
        }
    }
    out
}

fn greedy_ranges(widths: &[f32], max_width: f32) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut width = 0.0;
    for (index, token_width) in widths.iter().enumerate() {
        if width > 0.0 && width + token_width > max_width {
            ranges.push(start..index);
            start = index;
            width = 0.0;
        }
        width += token_width;
    }
    ranges.push(start..widths.len());
    ranges
}

/// 断句位置的语感代价：逗号后最好断，冠词/介词后最差。
fn break_penalty(tokens: &[LyricToken], next: usize) -> f32 {
    let previous = tokens[next - 1].text.trim_end();
    let following = tokens[next].text.trim_start();
    let mut penalty = 0.15;
    if previous.ends_with(|ch: char| {
        matches!(
            ch,
            ',' | ';' | ':' | '!' | '?' | '，' | '；' | '：' | '！' | '？' | '—' | '–'
        )
    }) {
        // 标点后的停顿最自然，给一点负代价主动吸引断点。
        penalty = -0.3;
    }
    if following.starts_with(|ch: char| {
        matches!(
            ch,
            ',' | '.'
                | ';'
                | ':'
                | '!'
                | '?'
                | ')'
                | ']'
                | '}'
                | '，'
                | '。'
                | '；'
                | '：'
                | '！'
                | '？'
                | '、'
        )
    }) {
        penalty += 4.0;
    }
    let word = previous
        .trim_matches(|ch: char| !ch.is_alphanumeric() && ch != '\'' && ch != '’')
        .to_lowercase();
    if matches!(
        word.as_str(),
        "the"
            | "a"
            | "an"
            | "of"
            | "in"
            | "on"
            | "at"
            | "to"
            | "with"
            | "and"
            | "but"
            | "or"
            | "for"
            | "from"
            | "by"
            | "as"
            | "my"
            | "your"
            | "his"
            | "her"
            | "their"
            | "our"
            | "that"
            | "when"
            | "while"
            | "if"
            | "until"
            | "because"
    ) {
        penalty += 1.2;
    }
    penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discrete_highlights_only_cover_complete_characters_and_words() {
        let tokens = lyric_tokens("远方 hello");
        assert_eq!(sung_prefix_bytes(&tokens, &mut 0.0), 0);
        assert_eq!(sung_prefix_bytes(&tokens, &mut 0.5), "远".len());
        assert_eq!(sung_prefix_bytes(&tokens, &mut 3.0), "远方 ".len());
        assert_eq!(sung_prefix_bytes(&tokens, &mut 4.5), "远方 hello".len());
    }

    #[test]
    fn stagger_follows_actual_scroll_and_far_lines_release_later() {
        assert_eq!(stagger_offset(0.1, 0.0, 3), 0.0);
        assert!(stagger_offset(0.1, 50.0, 3) > stagger_offset(0.1, 50.0, 1));
        assert_eq!(stagger_offset(0.5, 50.0, 6), 0.0);
    }

    #[test]
    fn perspective_keeps_center_full_size_and_recovers_symmetrically() {
        assert_eq!(lyric_perspective(0.0, 400.0), (1.0, 0.0, 0.0));
        let above = lyric_perspective(-400.0, 400.0);
        let below = lyric_perspective(400.0, 400.0);
        assert!(above.0 < 0.75 && above.1 > 30.0);
        assert_eq!(above, (below.0, below.1, -below.2));
    }

    /// 测试用等宽测量：每个字符 10px。
    fn mono(text: &str) -> f32 {
        text.chars().count() as f32 * 10.0
    }

    #[test]
    fn wrap_balances_two_lines_at_word_boundaries() {
        let tokens = lyric_tokens("I kept the windows open until the morning came");
        let ranges = wrap_tokens(&tokens, 240.0, &mono);
        assert_eq!(ranges.len(), 2, "两行足够时不要拆成三行");
        for range in &ranges {
            let width: f32 = tokens[range.clone()].iter().map(|t| mono(&t.text)).sum();
            assert!(width <= 240.0, "断行后不能溢出");
        }
        let first: String = tokens[ranges[0].clone()]
            .iter()
            .map(|token| token.text.as_str())
            .collect();
        let second: String = tokens[ranges[1].clone()]
            .iter()
            .map(|token| token.text.as_str())
            .collect();
        assert!(first.contains("windows"));
        assert!(second.trim_end().ends_with("came"));
        // 两行不能一边倒：短行至少占长行的一半。
        let first_w: f32 = tokens[ranges[0].clone()]
            .iter()
            .map(|t| mono(&t.text))
            .sum();
        let second_w: f32 = tokens[ranges[1].clone()]
            .iter()
            .map(|t| mono(&t.text))
            .sum();
        assert!(first_w.min(second_w) / first_w.max(second_w) > 0.5);
    }

    #[test]
    fn short_line_stays_single() {
        let tokens = lyric_tokens("Hold on");
        assert_eq!(wrap_tokens(&tokens, 240.0, &mono), vec![0..tokens.len()]);
    }

    #[test]
    fn lyric_hover_frame_distinguishes_active_and_tracks_3d_perspective() {
        // 不开立体：当前行更宽，其余行保持完全一致的宽度与高度。
        let active_2d = lyric_hover_frame(560.0, 64.0, 38.0, 1.0, 1.0, 0.0);
        let near_2d = lyric_hover_frame(560.0, 64.0, 38.0, 0.0, 1.0, 0.0);
        let far_2d = lyric_hover_frame(560.0, 64.0, 38.0, 0.0, 1.0, 0.0);
        assert!(active_2d.width > near_2d.width);
        assert_eq!(near_2d, far_2d);
        assert_eq!(active_2d.left, 0.0);
        assert_eq!(near_2d.left, 0.0);

        // 开启立体：越远缩放越小、左缩进越大，虚选中越窄且左缘紧跟文本缩进。
        let near_3d = lyric_hover_frame(560.0, 64.0, 38.0, 0.0, 0.94, 7.5);
        let far_3d = lyric_hover_frame(560.0, 64.0, 38.0, 0.0, 0.72, 34.7);
        assert!(near_2d.width > near_3d.width);
        assert!(near_3d.width > far_3d.width);
        assert!((far_3d.left - 34.7 * 0.72).abs() < 1e-4);
        assert!(far_3d.height < near_3d.height);
    }
}
