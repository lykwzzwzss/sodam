//! 左侧边栏：品牌、本地音乐、文件夹歌单。
//!
//! 对齐汽水音乐：256 宽、实底 base-7、导航行 44、分组标题 13/500、行悬停 .05 白。
//! 歌单列表可滚动。
//!
//! 选中高亮是一层共享背景：换歌单时从旧行滑到新行（260ms ease-in-out），
//! 文字/图标颜色与高亮同时开始过渡；悬停底色统一 120ms 淡入淡出。

use std::time::Instant;

use gpui::prelude::*;
use gpui::{div, px, svg, AnimationExt, AnyElement, ClickEvent, WindowControlArea};

use crate::app::{Root, SidebarRowAnim};
use crate::ui::motion::{self, HoverFade};
use crate::ui::{icons, theme};

/// 歌单行高与行间距：选中高亮按这两个值计算它应该滑到的位置。
const PLAYLIST_ROW_H: f32 = 40.0;
const PLAYLIST_ROW_GAP: f32 = theme::space::XS;

fn local_nav_row(
    label: String,
    selected: bool,
    fade: HoverFade,
    cx: &mut gpui::Context<Root>,
) -> AnyElement {
    div()
        .id("nav-local")
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::space::MD))
        .h(px(theme::size::NAV_ROW))
        .px(px(theme::space::MD))
        .rounded(px(theme::radius::ROW))
        .cursor_pointer()
        .when(!selected, |this| {
            this.child(motion::hover_layer(
                "nav-local-bg",
                fade,
                theme::radius::ROW,
                theme::surface_hover(),
            ))
        })
        .when(selected, |this| this.bg(theme::surface_selected()))
        // 始终监听悬停：选中的行不画悬停底色，但状态要保持准确，
        // 否则选中项换走后旧行会残留一层悬停色。
        .on_hover(Root::hover_listener(cx, "nav-local"))
        .text_size(theme::Text::Body.size())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(if selected {
            theme::text()
        } else {
            theme::text_muted()
        })
        .on_click(cx.listener(|root, _event: &ClickEvent, _window, cx| {
            root.settings_open = false;
            root.close_player_view(cx);
            cx.notify();
        }))
        .child(
            svg()
                .path(icons::path("folder-music"))
                .size(px(theme::ICON))
                .flex_none()
                .text_color(if selected {
                    theme::text()
                } else {
                    theme::text_muted()
                }),
        )
        .child(div().min_w(px(0.0)).truncate().child(label))
        .into_any_element()
}

fn playlist_row(
    index: usize,
    name: String,
    tracks: usize,
    is_all: bool,
    cover: Option<std::path::PathBuf>,
    anim: SidebarRowAnim,
    cx: &mut gpui::Context<Root>,
) -> AnyElement {
    // 悬停与选中共用一层背景，由同一个透明度插值，
    // 不会出现「hover + selected」两层叠加偏亮。
    let background = div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .rounded(px(theme::radius::ROW));
    let background = if anim.bg.seq == 0 || anim.bg.from == anim.bg.to {
        background
            .bg(theme::selection_tint(anim.bg.to))
            .into_any_element()
    } else {
        background
            .with_animation(
                gpui::ElementId::Name(format!("sidebar-row-bg-{index}-{}", anim.bg.seq).into()),
                motion::standard(anim.bg.duration),
                move |this, delta| {
                    let alpha = anim.bg.from + (anim.bg.to - anim.bg.from) * delta;
                    this.bg(theme::selection_tint(alpha))
                },
            )
            .into_any_element()
    };

    let row = div()
        .id(("local-playlist", index))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .h(px(PLAYLIST_ROW_H))
        .px(px(theme::space::MD))
        .rounded(px(theme::radius::ROW))
        .cursor_pointer()
        .text_size(theme::Text::Body.size())
        .font_weight(gpui::FontWeight::MEDIUM)
        .on_hover(cx.listener(move |root, hovered: &bool, _window, cx| {
            if root.set_sidebar_row_hover(index, *hovered) {
                cx.notify();
            }
        }))
        .on_click(cx.listener(move |root, _event: &ClickEvent, _window, cx| {
            root.close_player_view(cx);
            root.select_local_playlist(index, cx);
        }));

    // 封面/图标/文字各占一层：只有名字那一层做颜色动画。
    // 三者不能嵌套，否则外层动画换 ID 时，内层（尤其是封面的淡入）会被重建并重播。
    let leading = div().flex_none().flex().items_center().child(match cover {
        Some(path) => div()
            .size(px(24.0))
            .rounded(px(4.0))
            .overflow_hidden()
            .child(motion::thumb_img(path))
            .into_any_element(),
        None => {
            let icon = svg()
                .path(icons::path(if is_all { "disc-3" } else { "music" }))
                .size(px(theme::ICON_SM))
                .flex_none();
            if anim.text.seq == 0 || anim.text.from == anim.text.to {
                icon.text_color(motion::mix_rgba(
                    theme::text_muted(),
                    theme::text(),
                    anim.text.to,
                ))
                .into_any_element()
            } else {
                icon.with_animation(
                    gpui::ElementId::Name(
                        format!("sidebar-row-icon-{index}-{}", anim.text.seq).into(),
                    ),
                    motion::standard(anim.text.duration),
                    move |this, delta| {
                        let t = anim.text.from + (anim.text.to - anim.text.from) * delta;
                        this.text_color(motion::mix_rgba(theme::text_muted(), theme::text(), t))
                    },
                )
                .into_any_element()
            }
        }
    });

    let name = div().min_w(px(0.0)).truncate().child(name);
    let name = if anim.text.seq == 0 || anim.text.from == anim.text.to {
        name.text_color(motion::mix_rgba(
            theme::text_muted(),
            theme::text(),
            anim.text.to,
        ))
        .into_any_element()
    } else {
        name.with_animation(
            gpui::ElementId::Name(format!("sidebar-row-text-{index}-{}", anim.text.seq).into()),
            motion::standard(anim.text.duration),
            move |this, delta| {
                let t = anim.text.from + (anim.text.to - anim.text.from) * delta;
                this.text_color(motion::mix_rgba(theme::text_muted(), theme::text(), t))
            },
        )
        .into_any_element()
    };

    row.child(background)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::space::MD))
                .flex_1()
                .min_w(px(0.0))
                .child(leading)
                .child(div().flex_1().min_w(px(0.0)).child(name))
                .child(
                    div()
                        .flex_none()
                        .text_align(gpui::TextAlign::Right)
                        .text_size(theme::Text::Tiny.size())
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(theme::text_faint())
                        .child(tracks.to_string()),
                ),
        )
        .into_any_element()
}

impl Root {
    /// 指针进入/离开某一行：只改这一行的背景目标，选中行保持 13% 白。
    pub(crate) fn set_sidebar_row_hover(&mut self, index: usize, hovered: bool) -> bool {
        let selected = self.sidebar_selection_visible && self.local_selected == index;
        let anim = self.sidebar_row_anim_mut(index, selected);
        anim.set_hovered(hovered, selected, Instant::now())
    }

    fn row_hovered(&self, index: usize) -> bool {
        self.sidebar_rows
            .get(&index)
            .is_some_and(|anim| anim.hovered)
    }

    /// 选中歌单变化：旧行背景/文字淡出，新行淡入；两边各自从当前值续接。
    fn animate_sidebar_selection(&mut self, old: usize, new: usize, now: Instant) {
        if old == new {
            return;
        }
        let old_hovered = self.row_hovered(old);
        let old = self.sidebar_row_anim_mut(old, false);
        old.bg.retarget(
            if old_hovered { 0.07 } else { 0.0 },
            motion::duration::SIDEBAR_BG_OUT,
            now,
        );
        old.text
            .retarget(0.0, motion::duration::SIDEBAR_TEXT_OUT, now);

        let new_anim = self.sidebar_row_anim_mut(new, true);
        new_anim
            .bg
            .retarget(0.13, motion::duration::SIDEBAR_BG_IN, now);
        new_anim
            .text
            .retarget(1.0, motion::duration::SIDEBAR_TEXT_IN, now);
    }

    /// 播放页打开/关闭：选中行淡出/淡入，不保留共享高亮。
    fn set_sidebar_selection_visible(&mut self, visible: bool, now: Instant) {
        let index = self.sidebar_selection_shown;
        let hovered = self.row_hovered(index);
        let anim = self.sidebar_row_anim_mut(index, visible);
        if visible {
            anim.bg.retarget(0.13, motion::duration::SIDEBAR_BG_IN, now);
            anim.text
                .retarget(1.0, motion::duration::SIDEBAR_TEXT_IN, now);
        } else {
            anim.bg.retarget(
                if hovered { 0.07 } else { 0.0 },
                motion::duration::SIDEBAR_BG_OUT,
                now,
            );
            anim.text
                .retarget(0.0, motion::duration::SIDEBAR_TEXT_OUT, now);
        }
    }

    fn sidebar_row_anim_mut(&mut self, index: usize, selected: bool) -> &mut SidebarRowAnim {
        self.sidebar_rows
            .entry(index)
            .or_insert_with(|| SidebarRowAnim::new(selected, false))
    }
}

/// 左侧细条：相邻行滑动，远距离旧条淡出、新条淡入。
fn sidebar_marker(from: usize, to: usize, seq: u64) -> AnyElement {
    const MARKER_W: f32 = 3.0;
    const MARKER_H: f32 = 18.0;
    let row_step = PLAYLIST_ROW_H + PLAYLIST_ROW_GAP;
    let marker_top = |index: usize| index as f32 * row_step + (PLAYLIST_ROW_H - MARKER_H) * 0.5;
    let from_top = marker_top(from);
    let to_top = marker_top(to);
    let bar = |top: f32| {
        div()
            .absolute()
            .left(px(2.0))
            .top(px(top))
            .w(px(MARKER_W))
            .h(px(MARKER_H))
            .rounded(px(MARKER_W * 0.5))
            .bg(theme::accent())
    };

    if seq == 0 || from == to {
        return bar(to_top).into_any_element();
    }
    if from.abs_diff(to) <= 2 {
        return bar(from_top)
            .with_animation(
                gpui::ElementId::Name(format!("sidebar-marker-{seq}").into()),
                motion::standard(motion::duration::SIDEBAR_MARKER_NEAR),
                move |this, delta| this.top(px(from_top + (to_top - from_top) * delta)),
            )
            .into_any_element();
    }

    let old = bar(from_top)
        .with_animation(
            gpui::ElementId::Name(format!("sidebar-marker-out-{seq}").into()),
            motion::standard(motion::duration::SIDEBAR_MARKER_FAR),
            |this, delta| this.opacity(1.0 - delta),
        )
        .into_any_element();
    let new = bar(to_top)
        .with_animation(
            gpui::ElementId::Name(format!("sidebar-marker-in-{seq}").into()),
            motion::standard(motion::duration::SIDEBAR_MARKER_FAR),
            |this, delta| this.opacity(delta),
        )
        .into_any_element();
    div()
        .absolute()
        .size_full()
        .child(old)
        .child(new)
        .into_any_element()
}

fn section_label(text: &str) -> AnyElement {
    div()
        .px(px(theme::space::MD))
        .pt(px(theme::space::LG))
        .pb(px(theme::space::XS))
        .text_size(theme::Text::Small.size())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme::text_faint())
        .child(text.to_string())
        .into_any_element()
}

pub fn render(
    root: &mut Root,
    window: &gpui::Window,
    cx: &mut gpui::Context<Root>,
    own_titlebar: bool,
) -> impl IntoElement {
    let logo_side = 30.0;
    let device_side = (logo_side * window.scale_factor()).round().max(1.0) as i32;
    if root.sidebar_logo.as_ref().map(|(side, _)| *side) != Some(device_side) {
        root.sidebar_logo = icons::brand_logo(cx, device_side).map(|image| (device_side, image));
    }
    let logo = root.sidebar_logo.as_ref().map(|(_, image)| image.clone());
    let logo_extent = px(device_side as f32 / window.scale_factor());
    let now = Instant::now();
    let visible = !root.player_view_open && !root.settings_open;
    let selection_changed = root.sidebar_selection_shown != root.local_selected;
    if selection_changed {
        let old = root.sidebar_selection_shown;
        root.sidebar_selection_shown = root.local_selected;
        root.sidebar_marker_from = old;
        root.sidebar_marker_seq = root.sidebar_marker_seq.wrapping_add(1);
        root.animate_sidebar_selection(old, root.local_selected, now);
    }
    if root.sidebar_selection_visible != visible {
        root.sidebar_selection_visible = visible;
        if !selection_changed {
            root.set_sidebar_selection_visible(visible, now);
        }
    }
    let local_selected = visible;
    let nav_fade = root.hovers.get("nav-local");
    let playlist_count = root
        .local_library
        .as_ref()
        .map(|library| library.playlists.len())
        .unwrap_or(0);
    let playlists: Vec<(usize, String, usize, bool, Option<std::path::PathBuf>)> = root
        .local_library
        .as_ref()
        .map(|library| {
            library
                .playlists
                .iter()
                .enumerate()
                .map(|(index, playlist)| {
                    (
                        index,
                        root.local_playlist_name(playlist),
                        playlist.tracks.len(),
                        playlist.kind == sodam_core::local::LocalPlaylistKind::All,
                        root.playlist_cover(playlist),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let playlists: Vec<AnyElement> = playlists
        .into_iter()
        .map(|(index, name, tracks, is_all, cover)| {
            let selected = local_selected && root.local_selected == index;
            let anim = root.sidebar_row_anim_mut(index, selected);
            anim.bg.settle(now);
            anim.text.settle(now);
            let anim = *anim;
            playlist_row(index, name, tracks, is_all, cover, anim, cx)
        })
        .collect();
    let marker_from = root
        .sidebar_marker_from
        .min(playlist_count.saturating_sub(1));
    let selection_marker = (local_selected && playlist_count > 0).then(|| {
        sidebar_marker(
            marker_from,
            root.local_selected.min(playlist_count - 1),
            root.sidebar_marker_seq,
        )
    });
    let brand = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::space::MD))
        .h(px(72.0))
        .px(px(theme::space::MD))
        .flex_none();
    // 品牌行是窗口拖拽区（Windows 自绘标题栏）；没有可点击子元素，不会抢命中。
    let brand = if own_titlebar {
        brand.window_control_area(WindowControlArea::Drag)
    } else {
        brand
    };

    div()
        .flex()
        .flex_col()
        .w(px(theme::SIDEBAR_W))
        .h_full()
        .flex_none()
        // 半透明磨砂底：整页色场会从侧栏透上来。背后只有平滑色场，
        // 不再叠 backdrop_blur——模糊它没有视觉收益，只多一遍全屏采样。
        .bg(theme::chrome())
        .border_r_1()
        .border_color(theme::border())
        .px(px(theme::space::LG))
        .when(cfg!(target_os = "macos"), |this| this.pt(px(28.0)))
        .child(
            brand
                .child(match logo {
                    Some(image) => gpui::img(image).size(logo_extent).into_any_element(),
                    None => gpui::img("icons/sodam-logo.png")
                        .size(logo_extent)
                        .into_any_element(),
                })
                .child(
                    div()
                        .text_size(px(22.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child("SodaM"),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(theme::space::XS))
                .child(local_nav_row(
                    root.tr("本地音乐").to_string(),
                    local_selected,
                    nav_fade,
                    cx,
                )),
        )
        .child(section_label(&format!(
            "{} · {}",
            root.tr("歌单"),
            playlist_count
        )))
        .child(
            crate::app_scroll::scroll_area(
                "local-playlists-scroll",
                cx,
                root.sidebar_scroll.clone(),
            )
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(theme::space::XS))
            .children(playlists)
            .when_some(selection_marker, |this, marker| this.child(marker)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .pt(px(theme::space::SM))
                .pb(px(theme::space::MD))
                .border_t_1()
                .border_color(theme::border())
                .child({
                    let settings_selected = root.settings_open && !root.player_view_open;
                    let settings_fade = root.hovers.get("nav-settings");
                    div()
                        .id("nav-settings")
                        .relative()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::space::MD))
                        .h(px(PLAYLIST_ROW_H))
                        .px(px(theme::space::MD))
                        .rounded(px(theme::radius::ROW))
                        .cursor_pointer()
                        .when(!settings_selected, |this| {
                            this.child(motion::hover_layer(
                                "nav-settings-bg",
                                settings_fade,
                                theme::radius::ROW,
                                theme::surface_hover(),
                            ))
                        })
                        .when(settings_selected, |this| {
                            this.bg(theme::surface_selected()).child(
                                div()
                                    .absolute()
                                    .left(px(2.0))
                                    .top(px((PLAYLIST_ROW_H - 18.0) * 0.5))
                                    .w(px(3.0))
                                    .h(px(18.0))
                                    .rounded(px(1.5))
                                    .bg(theme::accent()),
                            )
                        })
                        .on_hover(Root::hover_listener(cx, "nav-settings"))
                        .text_size(theme::Text::Body.size())
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if settings_selected {
                            theme::text()
                        } else {
                            theme::text_muted()
                        })
                        .on_click(cx.listener(|root, _: &ClickEvent, _window, cx| {
                            root.player_view_open = false;
                            root.settings_open = true;
                            cx.notify();
                        }))
                        .child(
                            svg()
                                .path(icons::path("settings"))
                                .size(px(theme::ICON))
                                .flex_none()
                                .text_color(if settings_selected {
                                    theme::text()
                                } else {
                                    theme::text_muted()
                                }),
                        )
                        .child(root.tr("设置").to_string())
                }),
        )
}
