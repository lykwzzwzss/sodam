//! 顶部工具条：窗口拖拽区、当前音乐文件夹胶囊、扫描/换文件夹、
//! 主题切换、选择文件夹与窗口控制。
//!
//! 对齐汽水音乐：高 60，居中一个 320×34 的胶囊，右侧是白色主按钮和窗口控制。
//! 展开「正在播放」时换成「返回 + 正在播放」的简洁头。

use gpui::prelude::*;
use gpui::{
    canvas, div, px, svg, AnimationExt, AnyElement, ClickEvent, Context, IntoElement, MouseButton,
    MouseDownEvent, Window,
};

use crate::app::Root;
use crate::ui::motion::{self, HoverFade};
use crate::ui::{icons, theme, titlebar};

pub(crate) fn render(
    root: &mut Root,
    window: &Window,
    cx: &mut Context<Root>,
    own_titlebar: bool,
) -> impl IntoElement {
    let back_fade = root.hovers.get("player-back");
    let rescan_fade = root.hovers.get("topbar-rescan");
    let folder_fade = root.hovers.get("topbar-local-folder");
    let taskbar_fade = root.hovers.get("topbar-taskbar-player");
    let taskbar_on = root.settings.taskbar_player;
    let mut bar = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(theme::TOOLBAR_H))
        .w_full()
        .flex_none()
        .pl(px(theme::space::LG))
        .pr(px(0.0))
        // 顶栏是半透明黑条（汽水音乐同款）；下方内容滚动时会从这里透出来。
        .bg(gpui::rgba(0x00000030))
        .border_b_1()
        .border_color(theme::border());

    if root.player_view_open {
        bar = bar
            .child(round_icon_button(
                "player-back",
                "chevron-left",
                theme::ICON,
                back_fade,
                cx,
                |root, cx| {
                    root.close_player_view(cx);
                },
            ))
            .child(
                div()
                    .ml(px(theme::space::SM))
                    .flex_none()
                    .text_size(theme::Text::Body.size())
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme::text_muted())
                    .child(root.tr("正在播放")),
            )
            .child(titlebar::drag_area(root.titlebar_anchor.clone()));
        // 译文开关放在顶栏这条独立图层里：自己的磨砂底 + 命中区，
        // 不覆盖整行歌词，也不会被下面的滚动区截走点击。
        if let Some(toggle) = translation_toggle(root, cx) {
            bar = bar.child(toggle);
        }
        bar = bar.child(if own_titlebar {
            titlebar::controls(root, window, cx).into_any_element()
        } else {
            div().w(px(theme::space::LG)).into_any_element()
        });
        return bar;
    }

    let has_library = !root.settings.local_music_folder.trim().is_empty();

    // 左右两个弹性拖拽区把搜索框推到视觉中心；它们与按钮是兄弟节点，
    // 这样按钮/输入框的命中区不会被拖拽区抢走。
    bar = bar
        .child(titlebar::drag_area(root.titlebar_anchor.clone()))
        .child(search_box(root, window, cx))
        .child(titlebar::drag_area(root.titlebar_anchor.clone()));

    if has_library {
        bar = bar.child(round_icon_button(
            "topbar-rescan",
            "refresh",
            ACTION_ICON,
            rescan_fade,
            cx,
            |root, cx| {
                root.scan_local_library(cx);
            },
        ));
    }

    bar = bar
        .child(round_icon_button(
            "topbar-local-folder",
            "folder-plus",
            ACTION_ICON,
            folder_fade,
            cx,
            |root, cx| root.pick_local_folder(cx),
        ))
        .when(cfg!(target_os = "windows"), |this| {
            this.child(round_icon_button_colored(
                if taskbar_on {
                    theme::accent()
                } else {
                    theme::text_muted()
                },
                "topbar-taskbar-player",
                "taskbar",
                ACTION_ICON,
                taskbar_fade,
                cx,
                |root, cx| root.toggle_taskbar_player(cx),
            ))
        })
        .child(if own_titlebar {
            titlebar::controls(root, window, cx).into_any_element()
        } else {
            div().w(px(theme::space::LG)).into_any_element()
        });

    bar
}

/// 顶栏应用动作图标的统一尺寸：刷新与文件夹共用同一高度和线宽。
///
/// 比窗口控制小一档：它们是次要动作，不该和搜索框/窗口按钮抢注意力。
const ACTION_ICON: f32 = 18.0;

fn icon(name: &'static str, size: f32, color: gpui::Rgba) -> impl IntoElement {
    svg()
        .path(icons::path(name))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

fn search_box(root: &mut Root, window: &Window, cx: &mut Context<Root>) -> impl IntoElement {
    use crate::shortcuts::INPUT_CONTEXT;

    let focused = root.search_focus.is_focused(window);
    // 焦点变化只在真正切换时换代次；首帧不播「从强调色淡回普通描边」。
    if root.search_focus_shown != focused {
        root.search_focus_shown = focused;
        root.search_focus_seq = root.search_focus_seq.wrapping_add(1);
    }
    let focus_seq = root.search_focus_seq;
    let clear_fade = root.hovers.get("topbar-search-clear");
    let query = root.search_field.text().to_string();
    let empty = query.is_empty();
    let focus = root.search_focus.clone();
    let entity = cx.entity();
    let bounds_slot = root.search_bounds.clone();

    let search = div()
        .id("topbar-search")
        .relative()
        .track_focus(&root.search_focus)
        .key_context(INPUT_CONTEXT)
        .flex()
        .items_center()
        .flex_none()
        .w(px(420.0))
        .h(px(36.0))
        .px(px(theme::space::MD))
        .rounded(px(theme::radius::CARD))
        .bg(theme::surface_elevated())
        .border_1()
        .border_color(theme::border())
        .cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|root, _event: &MouseDownEvent, window, cx| {
                window.focus(&root.search_focus, cx);
                cx.notify();
            }),
        )
        .on_action(cx.listener(Root::text_backspace))
        .on_action(cx.listener(Root::text_delete))
        .on_action(cx.listener(Root::text_delete_word_backward))
        .on_action(cx.listener(Root::text_left))
        .on_action(cx.listener(Root::text_right))
        .on_action(cx.listener(Root::text_word_left))
        .on_action(cx.listener(Root::text_word_right))
        .on_action(cx.listener(Root::text_select_left))
        .on_action(cx.listener(Root::text_select_right))
        .on_action(cx.listener(Root::text_select_word_left))
        .on_action(cx.listener(Root::text_select_word_right))
        .on_action(cx.listener(Root::text_home))
        .on_action(cx.listener(Root::text_end))
        .on_action(cx.listener(Root::text_select_home))
        .on_action(cx.listener(Root::text_select_end))
        .on_action(cx.listener(Root::text_select_all))
        .on_action(cx.listener(Root::text_copy))
        .on_action(cx.listener(Root::text_cut))
        .on_action(cx.listener(Root::text_paste))
        .on_action(cx.listener(Root::text_undo))
        .on_action(cx.listener(Root::text_redo))
        .on_action(cx.listener(Root::text_submit))
        .on_action(cx.listener(Root::text_cancel))
        .on_action(cx.listener(Root::focus_search))
        // 输入法/字符输入必须在 paint 阶段注册；这个 canvas 要放在最底层，
        // 否则会盖住右侧的清除按钮，点击清不掉搜索词。
        .child(
            canvas(
                move |bounds: gpui::Bounds<gpui::Pixels>, _window, _cx| {
                    bounds_slot.set(Some(bounds));
                },
                move |bounds, _state, window, cx| {
                    window.handle_input(
                        &focus,
                        gpui::ElementInputHandler::new(bounds, entity.clone()),
                        cx,
                    );
                },
            )
            .absolute()
            .size_full(),
        )
        .child(icon("search", theme::ICON_SM, theme::text_faint()))
        .child(
            div()
                .ml(px(theme::space::SM))
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(theme::Text::Body.size())
                .text_color(if empty {
                    theme::text_faint()
                } else {
                    theme::text()
                })
                .child(if empty {
                    root.tr("搜索本地音乐").to_string()
                } else {
                    query
                }),
        )
        .when(!empty, |this| {
            this.child(
                div()
                    .id("topbar-search-clear")
                    .relative()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.0))
                    .rounded_full()
                    .cursor_pointer()
                    .child(motion::hover_layer(
                        "search-clear-bg",
                        clear_fade,
                        999.0,
                        theme::surface_hover(),
                    ))
                    .on_hover(Root::hover_listener(cx, "topbar-search-clear"))
                    .on_click(cx.listener(|root, _event: &ClickEvent, window, cx| {
                        root.search_field.clear();
                        window.focus(&root.search_focus, cx);
                        cx.notify();
                    }))
                    .child(icon("x", 14.0, theme::text_faint()))
                    // 清除按钮第一次出现：120ms 淡入 + 0.92→1 放大。
                    .with_animation(
                        "search-clear-in",
                        motion::standard(motion::duration::HOVER),
                        |this, delta| this.opacity(delta).layer_scale(0.92 + 0.08 * delta),
                    ),
            )
        });

    // 焦点描边/底色 140ms 过渡：只改绘制样式，不改变布局。
    if focus_seq == 0 {
        search
            .border_color(if focused {
                theme::accent()
            } else {
                theme::border()
            })
            .into_any_element()
    } else {
        let (border_from, border_to) = if focused {
            (theme::border(), theme::accent())
        } else {
            (theme::accent(), theme::border())
        };
        let (bg_from, bg_to) = if focused {
            (theme::surface_elevated(), gpui::rgba(0xFFFFFF12))
        } else {
            (gpui::rgba(0xFFFFFF12), theme::surface_elevated())
        };
        search
            .with_animation(
                gpui::ElementId::Name(format!("search-focus-{focus_seq}").into()),
                motion::standard(motion::duration::FOCUS),
                move |this, delta| {
                    this.border_color(motion::mix_rgba(border_from, border_to, delta))
                        .bg(motion::mix_rgba(bg_from, bg_to, delta))
                },
            )
            .into_any_element()
    }
}

/// 播放页顶栏的译文开关：只在当前歌词确实有翻译时出现。
///
/// 它是一颗独立的磨砂胶囊（backdrop blur + 半透明底），不是盖在歌词上的
/// 整行遮罩；作为顶栏的正常子元素，命中区稳定，不会被歌词滚动区穿透。
fn translation_toggle(root: &mut Root, cx: &mut Context<Root>) -> Option<AnyElement> {
    let has_translation = root.lyrics.as_ref().is_some_and(|lyrics| {
        lyrics.lines.iter().any(|line| {
            line.translation
                .as_deref()
                .is_some_and(|translation| !translation.trim().is_empty())
        })
    });
    if !has_translation {
        return None;
    }
    let on = root.settings.lyrics_translation;
    let fade = root.hovers.get("lyrics-translation");
    let accent = theme::accent();
    Some(
        div()
            .id("lyrics-translation")
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(theme::space::XS))
            .h(px(28.0))
            .px(px(theme::space::MD))
            .mr(px(theme::space::SM))
            .rounded(px(theme::radius::PILL))
            .cursor_pointer()
            .bg(if on {
                motion::with_alpha(accent, 0.18)
            } else {
                theme::glass_panel()
            })
            .backdrop_blur(px(18.0))
            .border_1()
            .border_color(if on {
                motion::with_alpha(accent, 0.36)
            } else {
                theme::border_strong()
            })
            .text_size(theme::Text::Tiny.size())
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(if on { accent } else { theme::text_muted() })
            .child(motion::hover_layer(
                "lyrics-translation-bg",
                fade,
                theme::radius::PILL,
                theme::surface_hover(),
            ))
            .active(|style| style.layer_scale(0.96))
            .on_hover(Root::hover_listener(cx, "lyrics-translation"))
            .on_click(cx.listener(|root, _event, _window, cx| {
                root.toggle_lyrics_translation(cx);
            }))
            .child(root.tr("译文"))
            .child(root.tr(if on { "开" } else { "关" }))
            .into_any_element(),
    )
}

fn round_icon_button(
    id: &'static str,
    icon_name: &'static str,
    icon_size: f32,
    fade: HoverFade,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    round_icon_button_colored(
        theme::text_muted(),
        id,
        icon_name,
        icon_size,
        fade,
        cx,
        on_click,
    )
}

fn round_icon_button_colored(
    color: gpui::Rgba,
    id: &'static str,
    icon_name: &'static str,
    icon_size: f32,
    fade: HoverFade,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(30.0))
        .ml(px(theme::space::SM))
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
            on_click(root, cx);
            cx.notify();
        }))
        .child(icon(icon_name, icon_size, color))
}
