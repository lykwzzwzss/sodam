//! 设置页：语言、本地曲库、播放与歌词、快捷键与诊断日志。

use gpui::prelude::*;
use gpui::{div, px, AnyElement, ClickEvent, Context, MouseButton};

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

    let lyrics_translation = root.settings.lyrics_translation;
    let loudness_equalization = root.settings.loudness_equalization;
    let taskbar_player = root.settings.taskbar_player;
    let detailed_logs = root.settings.detailed_logs;

    let toggle_key = root.settings.shortcut_toggle().to_string();
    let prev_key = root.settings.shortcut_previous().to_string();
    let next_key = root.settings.shortcut_next().to_string();

    let sections = div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(theme::space::LG))
        // 1. 外观与语言
        .child(section_card(
            root.tr("外观与语言"),
            vec![setting_row(
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
            )],
        ))
        // 2. 本地曲库
        .child(section_card(
            root.tr("本地曲库"),
            vec![setting_row(
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
            )],
        ))
        // 3. 播放与歌词
        .child({
            let mut rows = vec![
                setting_row(
                    root.tr("双语歌词翻译"),
                    root.tr("在播放页显示第二行歌词译文"),
                    toggle_pill(
                        "settings-lyrics-tr",
                        lyrics_translation,
                        root.tr("开启"),
                        root.tr("关闭"),
                        cx,
                        |root, cx| {
                            root.settings.lyrics_translation = !root.settings.lyrics_translation;
                            save_settings(root, cx);
                        },
                    ),
                ),
                setting_row(
                    root.tr("响度均衡"),
                    root.tr("自动平衡不同歌曲的播放音量。"),
                    toggle_pill(
                        "settings-loudness-eq",
                        loudness_equalization,
                        root.tr("开启"),
                        root.tr("关闭"),
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
                                            .map(|t| {
                                                (
                                                    format!("local:{}", t.path.display()),
                                                    t.path.clone(),
                                                )
                                            })
                                            .collect();
                                        root.engine.loudness().enqueue_missing(items);
                                    }
                                }
                            }
                            save_settings(root, cx);
                        },
                    ),
                ),
            ];
            if cfg!(target_os = "windows") {
                rows.push(setting_row(
                    root.tr("任务栏快捷播控"),
                    root.tr("在 Windows 任务栏右侧嵌入迷你播放条"),
                    toggle_pill(
                        "settings-taskbar-player",
                        taskbar_player,
                        root.tr("开启"),
                        root.tr("关闭"),
                        cx,
                        |root, cx| {
                            root.settings.taskbar_player = !root.settings.taskbar_player;
                            save_settings(root, cx);
                        },
                    ),
                ));
            }
            section_card(root.tr("播放与歌词"), rows)
        })
        // 4. 快捷键
        .child(section_card(
            root.tr("快捷键"),
            vec![
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
        ))
        // 5. 诊断与关于
        .child(section_card(
            root.tr("诊断与关于"),
            vec![
                setting_row(
                    root.tr("详细日志"),
                    root.tr("记录更多调试信息到本地日志文件，重启保持"),
                    toggle_pill(
                        "settings-detailed-logs",
                        detailed_logs,
                        root.tr("开启"),
                        root.tr("关闭"),
                        cx,
                        |root, cx| {
                            root.settings.detailed_logs = !root.settings.detailed_logs;
                            sodam_core::log_outlet::set_global_detailed(
                                root.settings.detailed_logs,
                            );
                            save_settings(root, cx);
                        },
                    ),
                ),
                setting_row(
                    root.tr("应用版本"),
                    "SodaM — 纯本地音乐播放器",
                    key_badge(&format!("v{}", env!("CARGO_PKG_VERSION"))),
                ),
            ],
        ));

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .gap(px(theme::space::LG))
        .child(title)
        .child(
            crate::app_scroll::scroll_area("settings-scroll", cx, root.settings_scroll.clone())
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .pr(px(theme::space::XS))
                .child(sections),
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
        .bg(theme::chrome())
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
            this.bg(theme::accent())
                .text_color(theme::text())
                .font_weight(gpui::FontWeight::MEDIUM)
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

fn toggle_pill(
    id: &'static str,
    enabled: bool,
    on_label: &str,
    off_label: &str,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
) -> AnyElement {
    let label = if enabled { on_label } else { off_label };
    choice_pill(id, label, enabled, cx, on_click)
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
