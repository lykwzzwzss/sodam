//! 设置页：语言、本地曲库、播放与歌词、快捷键与诊断日志。

use gpui::prelude::*;
use gpui::{canvas, div, px, AnyElement, ClickEvent, Context, DragMoveEvent, MouseButton};
use sodam_core::config::{Settings, LYRICS_FONT_SIZE, LYRICS_FONT_WEIGHT};
use std::cell::Cell;
use std::rc::Rc;

use crate::app::Root;
use crate::ui::theme;

pub fn settings_view(root: &Root, cx: &mut Context<Root>) -> AnyElement {
    let title = super::page_title(root.tr("设置"));

    let language = root.settings.language.trim().to_ascii_lowercase();
    let lang_auto = language.is_empty() || language == "auto";
    let lang_zh = language == "zh";
    let lang_en = language == "en";

    let folder_text = if root.settings.local_music_folder.trim().is_empty() {
        root.tr("未选择文件夹").to_string()
    } else {
        root.settings.local_music_folder.clone()
    };

    let light_mode = root.settings.light_mode;
    let flowing_background = root.settings.flowing_background;
    let flowing_background_global = root.settings.flowing_background_global;
    let lyrics_blur = root.settings.lyrics_blur;
    let lyrics_staggered = root.settings.lyrics_staggered_scroll;
    let lyrics_3d = root.settings.lyrics_3d_effect;
    let lyrics_continuous_sweep = root.settings.lyrics_continuous_sweep;
    let lyrics_translation = root.settings.lyrics_translation;
    let loudness_equalization = root.settings.loudness_equalization;
    let taskbar_player = root.settings.taskbar_player;
    let detailed_logs = root.settings.detailed_logs;
    let active_section = root.settings_active_section;

    let toggle_key = root.settings.shortcut_toggle().to_string();
    let prev_key = root.settings.shortcut_previous().to_string();
    let next_key = root.settings.shortcut_next().to_string();

    let nav_items: [(&'static str, &str); 5] = [
        ("settings-nav-appearance", root.tr("外观与主题")),
        ("settings-nav-lyrics", root.tr("歌词显示")),
        ("settings-nav-playback", root.tr("播放与音频")),
        ("settings-nav-library", root.tr("曲库与快捷键")),
        ("settings-nav-about", root.tr("诊断与关于")),
    ];
    let mut secondary_nav = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(theme::space::XS));
    for (idx, (id, label)) in nav_items.into_iter().enumerate() {
        let selected = active_section == idx;
        let (selected_bg, selected_text) = theme::selected_choice();
        secondary_nav = secondary_nav.child(
            div()
                .id(id)
                .px(px(theme::space::MD))
                .h(px(30.0))
                .flex()
                .items_center()
                .rounded(px(theme::radius::PILL))
                .bg(if selected {
                    selected_bg
                } else {
                    theme::surface_elevated()
                })
                .border_1()
                .border_color(if selected {
                    theme::accent()
                } else {
                    theme::border()
                })
                .text_size(theme::Text::Small.size())
                .font_weight(if selected {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if selected {
                    selected_text
                } else {
                    theme::text_muted()
                })
                .cursor_pointer()
                .when(!selected, |this| {
                    this.hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
                })
                .on_click(cx.listener(move |root, _event: &ClickEvent, window, cx| {
                    root.scroll_settings_to(idx, window, cx);
                }))
                .child(label.to_string()),
        );
    }

    // 1. 外观与主题
    let card_appearance = section_card(
        root.tr("外观与主题"),
        vec![
            setting_row(
                root.tr("浅色模式"),
                root.tr("切换深色/浅色界面外观，带平滑过渡动画"),
                toggle_switch("settings-light-mode", light_mode, cx, |root, cx| {
                    root.toggle_light_mode(cx);
                }),
            ),
            setting_row(
                root.tr("动态流光背景"),
                root.tr("根据封面色彩缓慢流动的氛围光效"),
                toggle_switch("settings-flowing-bg", flowing_background, cx, |root, cx| {
                    root.settings.flowing_background = !root.settings.flowing_background;
                    save_settings(root, cx);
                }),
            ),
            div()
                .w_full()
                .opacity(if flowing_background { 1.0 } else { 0.45 })
                .child(setting_row(
                    root.tr("全局应用流光"),
                    root.tr("开启后全界面共用流动光效，关闭时仅在歌词播放页展示"),
                    toggle_switch(
                        "settings-flowing-global",
                        flowing_background_global,
                        cx,
                        |root, cx| {
                            root.settings.flowing_background_global =
                                !root.settings.flowing_background_global;
                            save_settings(root, cx);
                        },
                    ),
                ))
                .into_any_element(),
            setting_row(
                root.tr("界面语言"),
                root.tr("切换应用界面显示语言，立即生效"),
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::space::XS))
                    .child(choice_pill(
                        "lang-auto",
                        root.tr("跟随系统"),
                        lang_auto,
                        cx,
                        |root, cx| {
                            root.settings.language = "auto".to_string();
                            save_settings(root, cx);
                        },
                    ))
                    .child(choice_pill(
                        "lang-zh",
                        "简体中文",
                        lang_zh,
                        cx,
                        |root, cx| {
                            root.settings.language = "zh".to_string();
                            save_settings(root, cx);
                        },
                    ))
                    .child(choice_pill(
                        "lang-en",
                        "English",
                        lang_en,
                        cx,
                        |root, cx| {
                            root.settings.language = "en".to_string();
                            save_settings(root, cx);
                        },
                    ))
                    .into_any_element(),
            ),
        ],
    );

    // 2. 歌词显示
    let card_lyrics = section_card(
        root.tr("歌词显示"),
        vec![
            setting_row(
                root.tr("连续扫色"),
                root.tr("开启后沿歌词平滑扫色，关闭时逐字点亮"),
                toggle_switch(
                    "settings-lyrics-continuous-sweep",
                    lyrics_continuous_sweep,
                    cx,
                    |root, cx| {
                        root.settings.lyrics_continuous_sweep =
                            !root.settings.lyrics_continuous_sweep;
                        save_settings(root, cx);
                    },
                ),
            ),
            setting_row(
                root.tr("歌词视图模糊"),
                root.tr("非当前行随距离渐隐柔化，突出正在演唱的一行"),
                toggle_switch("settings-lyrics-blur", lyrics_blur, cx, |root, cx| {
                    root.settings.lyrics_blur = !root.settings.lyrics_blur;
                    save_settings(root, cx);
                }),
            ),
            setting_row(
                root.tr("交错滚动效果"),
                root.tr("切换歌词行时按距离形成交错波浪滚动"),
                toggle_switch(
                    "settings-lyrics-stagger",
                    lyrics_staggered,
                    cx,
                    |root, cx| {
                        root.settings.lyrics_staggered_scroll =
                            !root.settings.lyrics_staggered_scroll;
                        save_settings(root, cx);
                    },
                ),
            ),
            setting_row(
                root.tr("立体歌词效果"),
                root.tr("远行歌词随距离微缩与内收，营造景深层次"),
                toggle_switch("settings-lyrics-3d", lyrics_3d, cx, |root, cx| {
                    root.settings.lyrics_3d_effect = !root.settings.lyrics_3d_effect;
                    save_settings(root, cx);
                }),
            ),
            setting_row(
                root.tr("歌词字体大小"),
                root.tr("调整播放页主歌词与译文的字号"),
                typography_slider(FontSlider::Size, root, cx),
            ),
            setting_row(
                root.tr("歌词字体粗细"),
                root.tr("调整播放页歌词的字重"),
                typography_slider(FontSlider::Weight, root, cx),
            ),
            setting_row(
                root.tr("双语歌词翻译"),
                root.tr("在播放页显示第二行歌词译文"),
                toggle_switch("settings-lyrics-tr", lyrics_translation, cx, |root, cx| {
                    root.settings.lyrics_translation = !root.settings.lyrics_translation;
                    save_settings(root, cx);
                }),
            ),
        ],
    );

    // 3. 播放与音频
    let mut playback_rows = vec![setting_row(
        root.tr("响度均衡"),
        root.tr("自动平衡不同歌曲的播放音量。"),
        toggle_switch(
            "settings-loudness-eq",
            loudness_equalization,
            cx,
            |root, cx| {
                let enabled = !root.settings.loudness_equalization;
                root.settings.loudness_equalization = enabled;
                root.engine.set_loudness_equalization(enabled);
                if enabled {
                    if let Some(library) = root.local_library.as_ref() {
                        if let Some(all) = library.playlists.first() {
                            let items = all
                                .tracks
                                .iter()
                                .map(|t| (format!("local:{}", t.path.display()), t.path.clone()))
                                .collect();
                            root.engine.loudness().enqueue_missing(items);
                        }
                    }
                }
                save_settings(root, cx);
            },
        ),
    )];
    if cfg!(target_os = "windows") {
        playback_rows.push(setting_row(
            root.tr("任务栏快捷播控"),
            root.tr("在 Windows 任务栏右侧嵌入迷你播放条"),
            toggle_switch("settings-taskbar-player", taskbar_player, cx, |root, cx| {
                root.toggle_taskbar_player(cx);
            }),
        ));
    }
    let card_playback = section_card(root.tr("播放与音频"), playback_rows);

    // 4. 曲库与快捷键
    let card_library_shortcuts = section_card(
        root.tr("曲库与快捷键"),
        vec![
            setting_row(
                root.tr("音乐文件夹"),
                &folder_text,
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::space::SM))
                    .child(action_button(
                        "settings-pick-folder",
                        root.tr("更改文件夹"),
                        cx,
                        |root, _window, cx| {
                            root.pick_local_folder(cx);
                        },
                    ))
                    .child(action_button(
                        "settings-rescan",
                        root.tr("重新扫描"),
                        cx,
                        |root, _window, cx| {
                            root.scan_local_library(cx);
                        },
                    ))
                    .into_any_element(),
            ),
            setting_row(
                root.tr("播放 / 暂停"),
                root.tr("全局播放控制快捷键"),
                key_badge(&toggle_key),
            ),
            setting_row(
                root.tr("上一首"),
                root.tr("切换到上一首曲目"),
                key_badge(&prev_key),
            ),
            setting_row(
                root.tr("下一首"),
                root.tr("切换到下一首曲目"),
                key_badge(&next_key),
            ),
        ],
    );

    // 5. 诊断与关于
    let card_about = section_card(
        root.tr("诊断与关于"),
        vec![
            setting_row(
                root.tr("详细日志"),
                root.tr("记录更多调试信息到本地日志文件，重启保持"),
                toggle_switch("settings-detailed-logs", detailed_logs, cx, |root, cx| {
                    root.settings.detailed_logs = !root.settings.detailed_logs;
                    sodam_core::log_outlet::set_global_detailed(root.settings.detailed_logs);
                    save_settings(root, cx);
                }),
            ),
            setting_row(
                root.tr("应用版本"),
                root.tr("SodaM — 纯本地音乐播放器"),
                key_badge(&format!("v{}", env!("CARGO_PKG_VERSION"))),
            ),
        ],
    );

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .gap(px(theme::space::LG))
        .child(title)
        .child(secondary_nav)
        .child(
            crate::app_scroll::scroll_area("settings-scroll", cx, root.settings_scroll.clone())
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .gap(px(theme::space::LG))
                .pr(px(theme::space::XS))
                .pb(px(theme::space::XL))
                .child(card_appearance)
                .child(card_lyrics)
                .child(card_playback)
                .child(card_library_shortcuts)
                .child(card_about),
        )
        .into_any_element()
}

fn section_card(title: &str, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .p(px(theme::space::LG))
        .gap(px(theme::space::MD))
        .rounded(px(theme::radius::CARD))
        .bg(theme::card_surface())
        .border_1()
        .border_color(theme::border())
        .child(
            div()
                .text_size(theme::Text::Body.size())
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(title.to_string()),
        )
        .children(rows)
        .into_any_element()
}

fn setting_row(label: &str, desc: &str, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .gap(px(theme::space::LG))
        .py(px(theme::space::XS))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(theme::Text::Body.size())
                        .text_color(theme::text())
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(theme::Text::Small.size())
                        .text_color(theme::text_muted())
                        .truncate()
                        .child(desc.to_string()),
                ),
        )
        .child(control)
        .into_any_element()
}

fn choice_pill(
    id: &'static str,
    label: &str,
    active: bool,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px(px(theme::space::MD))
        .py(px(6.0))
        .rounded(px(theme::radius::ROW))
        .cursor_pointer()
        .text_size(theme::Text::Small.size())
        .when(active, |this| {
            let (background, text) = theme::selected_choice();
            this.bg(background)
                .text_color(text)
                .font_weight(gpui::FontWeight::SEMIBOLD)
        })
        .when(!active, |this| {
            this.bg(theme::surface_hover())
                .text_color(theme::text_muted())
                .hover(|style| style.text_color(theme::text()))
        })
        .on_click(cx.listener(move |root, _: &ClickEvent, _window, cx| {
            on_click(root, cx);
        }))
        .child(label.to_string())
        .into_any_element()
}

fn toggle_switch(
    id: &'static str,
    enabled: bool,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .focusable()
        .flex()
        .flex_none()
        .items_center()
        .w(px(44.0))
        .h(px(24.0))
        .px(px(4.0))
        .when(enabled, |this| this.justify_end())
        .rounded_full()
        .bg(if enabled {
            theme::accent()
        } else {
            theme::switch_off_track()
        })
        .cursor_pointer()
        .active(|style| style.layer_scale(0.96))
        .child(div().size(px(16.0)).rounded_full().bg(theme::hex(0xFFFFFF)))
        .on_click(cx.listener(move |root, _: &ClickEvent, _window, cx| on_click(root, cx)))
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FontSlider {
    Size,
    Weight,
}

impl FontSlider {
    fn value(self, settings: &Settings) -> u16 {
        match self {
            Self::Size => settings.lyrics_font_size,
            Self::Weight => settings.lyrics_font_weight,
        }
    }

    fn set(self, settings: &mut Settings, value: u16) {
        match self {
            Self::Size => settings.lyrics_font_size = value,
            Self::Weight => settings.lyrics_font_weight = value,
        }
    }
}

fn typography_slider(field: FontSlider, root: &Root, cx: &mut Context<Root>) -> AnyElement {
    const WIDTH: f32 = 192.0;
    const THUMB: f32 = 12.0;
    let (id, range, step) = match field {
        FontSlider::Size => ("settings-lyrics-size", LYRICS_FONT_SIZE, 1),
        FontSlider::Weight => ("settings-lyrics-weight", LYRICS_FONT_WEIGHT, 100),
    };
    let (min, max) = (*range.start(), *range.end());
    let steps = (max - min) / step;
    let current = field.value(&root.settings);
    let fill = (WIDTH - THUMB) * f32::from(current - min) / f32::from(max - min);
    let bounds = Rc::new(Cell::new(None::<gpui::Bounds<gpui::Pixels>>));
    let measure = Rc::clone(&bounds);
    let update = move |root: &mut Root, left: f32, x: f32, cx: &mut Context<Root>| {
        let fraction = ((x - left - THUMB * 0.5) / (WIDTH - THUMB)).clamp(0.0, 1.0);
        let next = min + (fraction * f32::from(steps)).round() as u16 * step;
        if field.value(&root.settings) != next {
            field.set(&mut root.settings, next);
            cx.notify();
        }
    };
    let finish = move |root: &mut Root,
                       _: &gpui::MouseUpEvent,
                       _: &mut gpui::Window,
                       cx: &mut Context<Root>| {
        if root.settings_slider_drag == Some(id) {
            root.settings_slider_drag = None;
            save_settings(root, cx);
        }
    };

    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(theme::space::SM))
        .child(
            div()
                .id(id)
                .focusable()
                .relative()
                .flex_none()
                .w(px(WIDTH))
                .h(px(41.0))
                .cursor_pointer()
                .child(
                    div()
                        .absolute()
                        .left(px(THUMB * 0.5))
                        .top(px(12.0))
                        .w(px(WIDTH - THUMB))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(theme::surface_selected()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(THUMB * 0.5))
                        .top(px(12.0))
                        .w(px(fill))
                        .h(px(4.0))
                        .rounded_full()
                        .bg(theme::accent()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(fill))
                        .top(px(8.0))
                        .size(px(THUMB))
                        .rounded_full()
                        .bg(theme::text()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(THUMB * 0.5))
                        .bottom(px(0.0))
                        .w(px(WIDTH - THUMB))
                        .flex()
                        .justify_between()
                        .text_size(theme::Text::Tiny.size())
                        .text_color(theme::text_muted())
                        .child(min.to_string())
                        .child(max.to_string()),
                )
                .child(
                    canvas(move |rect, _, _| measure.set(Some(rect)), |_, _, _, _| {})
                        .absolute()
                        .size_full(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |root, event: &gpui::MouseDownEvent, _window, cx| {
                        if let Some(bounds) = bounds.get() {
                            root.settings_slider_drag = Some(id);
                            update(
                                root,
                                f32::from(bounds.left()),
                                f32::from(event.position.x),
                                cx,
                            );
                        }
                    }),
                )
                .on_drag(field, |_, _, _, cx| cx.new(|_| gpui::EmptyView))
                .on_drag_move(cx.listener(
                    move |root, event: &DragMoveEvent<FontSlider>, _window, cx| {
                        if *event.drag(cx) == field && root.settings_slider_drag == Some(id) {
                            update(
                                root,
                                f32::from(event.bounds.left()),
                                f32::from(event.event.position.x),
                                cx,
                            );
                        }
                    },
                ))
                .on_mouse_up(MouseButton::Left, cx.listener(finish))
                .on_mouse_up_out(MouseButton::Left, cx.listener(finish))
                .on_key_down(
                    cx.listener(move |root, event: &gpui::KeyDownEvent, _window, cx| {
                        let value = field.value(&root.settings);
                        let next = match event.keystroke.key.as_str() {
                            "left" | "down" => value.saturating_sub(step).max(min),
                            "right" | "up" => (value + step).min(max),
                            "home" => min,
                            "end" => max,
                            _ => return,
                        };
                        cx.stop_propagation();
                        if value != next {
                            field.set(&mut root.settings, next);
                            save_settings(root, cx);
                        }
                    }),
                ),
        )
        .child(
            div()
                .w(px(36.0))
                .self_start()
                .mt(px(6.0))
                .text_right()
                .text_size(theme::Text::Small.size())
                .text_color(theme::text())
                .child(current.to_string()),
        )
        .into_any_element()
}

fn action_button(
    id: &'static str,
    label: &str,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut gpui::Window, &mut Context<Root>) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px(px(theme::space::MD))
        .py(px(6.0))
        .rounded(px(theme::radius::ROW))
        .bg(theme::surface_hover())
        .text_size(theme::Text::Small.size())
        .text_color(theme::text())
        .cursor_pointer()
        .hover(|style| style.bg(theme::border()))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |root, _: &ClickEvent, window, cx| {
            on_click(root, window, cx);
        }))
        .child(label.to_string())
        .into_any_element()
}

fn key_badge(text: &str) -> AnyElement {
    div()
        .px(px(theme::space::SM))
        .py(px(4.0))
        .rounded(px(theme::radius::ROW))
        .bg(theme::surface_hover())
        .border_1()
        .border_color(theme::border())
        .text_size(theme::Text::Small.size())
        .text_color(theme::text_muted())
        .child(text.to_string())
        .into_any_element()
}

fn save_settings(root: &mut Root, cx: &mut Context<Root>) {
    if let Err(err) = root.settings.save() {
        root.set_status_text(err.to_string());
    }
    cx.notify();
}
