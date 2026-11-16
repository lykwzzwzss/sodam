//! 本地音乐页：歌单头图 + 曲目表。
//!
//! 布局对齐汽水音乐的播放列表页：左侧 200 封面 + 标题/歌数 + 白色「播放全部」，
//! 下方是 56 行高的歌曲表。数据来自 `Root::local_library`，动作全部转发到
//! `app_local.rs` 的 `impl Root` 方法，这里只负责画。

use super::*;
use crate::app::playing_strength;
use crate::ui::motion;

/// 本地音乐页入口。
pub(crate) fn local_view(root: &mut Root, cx: &mut Context<Root>) -> AnyElement {
    let folder = root.settings.local_music_folder.trim().to_string();
    let library = root.local_library.clone();
    // 扫描很快完成时 local_loading_visible 一直是 false，不会闪一下加载圈。
    let loading = root.local_loading_visible;
    let error = root.local_error.clone();
    let selected = root.local_selected;

    let mut column = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(px(theme::space::XL));

    if let Some(message) = error {
        column = column.child(
            div()
                .w_full()
                .px(px(theme::space::LG))
                .py(px(theme::space::MD))
                .rounded(px(theme::radius::CARD))
                .bg(theme::surface_elevated())
                .text_size(theme::Text::Small.size())
                .text_color(theme::danger())
                .child(message)
                // 错误条从上方 6px 淡入，出现时不会「啪」地压进来。
                .with_animation(
                    "local-error",
                    motion::standard(motion::duration::STATE),
                    |this, delta| this.opacity(delta).top(px(6.0 * (1.0 - delta))),
                ),
        );
    }

    if loading {
        column = column.child(
            div()
                .child(loading_state(root.tr("正在扫描本地音乐…")))
                .with_animation(
                    "local-loading",
                    motion::standard(motion::duration::STATE),
                    |this, delta| this.opacity(delta),
                ),
        );
    }

    // 搜索模式：顶栏输入即过滤整个本地库，不再显示歌单头图。
    let query = root.search_field.text().trim().to_string();
    if query.is_empty() && root.search_hover_query.is_some() {
        root.search_hover_query = None;
        root.search_hover.clear();
    }
    if !query.is_empty() {
        // 结果集变了，行号对应的歌也变了：旧的悬停行不能再保持高亮。
        if root.search_hover_query.as_deref() != Some(query.as_str()) {
            root.search_hover_query = Some(query.clone());
            root.search_hover.clear();
        }
        let results = root.search_results();
        root.request_cover_thumbs(
            results
                .iter()
                .take(80)
                .map(|track| track.path.clone())
                .collect(),
            cx,
        );
        let row_count = results.len();
        let entity = cx.entity();
        let scroll_handle = root.local_scroll.clone();
        let covers = root.cover_thumbs.clone();
        let hover_state = root.search_hover;
        let np_current = root.now_playing_fade.current.clone();
        let np_previous = root.now_playing_fade.previous.clone();
        let np_t = root.now_playing_fade.progress();
        let header = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::space::MD))
            .child(page_title(root.tr("搜索结果")))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(theme::Text::Small.size())
                    .text_color(theme::text_faint())
                    .child(query.clone()),
            )
            .child(
                div()
                    .text_size(theme::Text::Small.size())
                    .text_color(theme::text_muted())
                    .child(root.localized("{} 首", &[row_count.to_string()])),
            );
        let list = uniform_list(
            // 固定 id：逐字符过滤时列表整体重排，但不重播进入动画、不重置虚拟列表缓存。
            "local-search-list",
            row_count,
            move |range, _window, _cx| {
                range
                    .map(|index| {
                        let entity = entity.clone();
                        let hover_entity = entity.clone();
                        let track = &results[index];
                        let path_key = track.path.to_string_lossy().to_string();
                        let strength = playing_strength(
                            np_current.as_deref(),
                            np_previous.as_deref(),
                            np_t,
                            &path_key,
                        );
                        let cover = covers.get(&path_key).cloned().flatten();
                        let marker_color =
                            motion::mix_rgba(theme::text_faint(), theme::accent(), strength);
                        div()
                            .id(("local-search-row", index))
                            .relative()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(theme::space::MD))
                            .h(px(theme::size::LIST_ROW))
                            .px(px(theme::space::MD))
                            .rounded(px(theme::radius::ROW))
                            .cursor_pointer()
                            .when_some(
                                motion::row_hover_layer(
                                    "search-row",
                                    hover_state.fade_for(index),
                                    theme::radius::ROW,
                                    theme::surface_hover(),
                                ),
                                |this, layer| this.child(layer),
                            )
                            .on_hover(move |hovered: &bool, _window, cx: &mut gpui::App| {
                                let next = if *hovered { Some(index) } else { None };
                                hover_entity.update(cx, |root, cx| {
                                    let changed = match next {
                                        Some(_) => root.search_hover.set(next),
                                        None => {
                                            root.search_hover.current == Some(index)
                                                && root.search_hover.set(None)
                                        }
                                    };
                                    if changed {
                                        cx.notify();
                                    }
                                });
                            })
                            .on_click(move |_event: &ClickEvent, _window, cx: &mut gpui::App| {
                                entity.update(cx, |root, cx| {
                                    root.play_search_track(index, cx);
                                });
                            })
                            .child(
                                div()
                                    .relative()
                                    .w(px(28.0))
                                    .h(px(18.0))
                                    .flex_none()
                                    .text_size(theme::Text::Small.size())
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(0.0))
                                            .left(px(0.0))
                                            .opacity(1.0 - strength)
                                            .text_color(theme::text_faint())
                                            .child((index + 1).to_string()),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .top(px(0.0))
                                            .left(px(0.0))
                                            .opacity(strength)
                                            .text_color(marker_color)
                                            .child("♪"),
                                    ),
                            )
                            .child(match cover {
                                Some(path) => div()
                                    .flex_none()
                                    .size(px(theme::size::ROW_ART))
                                    .rounded(px(4.0))
                                    .overflow_hidden()
                                    .child(motion::thumb_img(path))
                                    .into_any_element(),
                                None => div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(theme::size::ROW_ART))
                                    .rounded(px(4.0))
                                    .bg(theme::surface_elevated())
                                    .child(
                                        svg()
                                            .path(icons::path("music"))
                                            .size(px(theme::ICON_SM))
                                            .text_color(theme::text_faint()),
                                    )
                                    .into_any_element(),
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_size(theme::Text::Body.size())
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme::text())
                                    .child(track.title.clone()),
                            )
                            .child(
                                div()
                                    .w(px(200.0))
                                    .flex_none()
                                    .truncate()
                                    .text_size(theme::Text::Small.size())
                                    .text_color(theme::text_muted())
                                    .child(track.artist.clone()),
                            )
                            .child(
                                div()
                                    .w(px(64.0))
                                    .flex_none()
                                    .text_right()
                                    .text_size(theme::Text::Small.size())
                                    .text_color(theme::text_faint())
                                    .child(local_duration(track.duration_seconds)),
                            )
                    })
                    .collect::<Vec<_>>()
            },
        )
        .size_full()
        .scrollbar_width(px(0.0))
        .track_scroll(&scroll_handle)
        .on_scroll_wheel(crate::app_scroll::uniform_wheel_listener(
            cx,
            "local-search-list",
            scroll_handle.clone(),
        ));
        // 歌单页 → 搜索页是结构变化：标题区域 220ms 淡入 + 下移归位；
        // 逐字符过滤期间元素 id 不变，不会跟着输入抖动。
        column = column.child(header.with_animation(
            "search-header",
            motion::standard(motion::duration::SEARCH),
            |this, delta| this.opacity(delta).top(px(6.0 * (1.0 - delta))),
        ));
        let body: AnyElement = if row_count == 0 {
            div()
                .child(empty_state("search", root.tr("没有找到本地音乐")))
                .with_animation(
                    "search-empty",
                    motion::standard(motion::duration::STATE),
                    |this, delta| this.opacity(delta),
                )
                .into_any_element()
        } else {
            fill_remaining(list.into_any_element())
        };
        column = column.child(
            div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .child(body)
                .with_animation(
                    "search-body",
                    motion::standard(motion::duration::SEARCH),
                    |this, delta| this.opacity(delta).top(px(6.0 * (1.0 - delta))),
                ),
        );
        return column.into_any_element();
    }

    match library {
        Some(library) if !library.playlists.is_empty() => {
            let selected_index = selected.min(library.playlists.len().saturating_sub(1));
            let current = &library.playlists[selected_index];
            let playlist_name = root.local_playlist_name(current);
            let playlist_count = current.tracks.len();
            // 标题/歌数/头图共用同一次切换事务的起点与序号。
            let (old_name, old_count, text_seq) = {
                let transition = root.playlist_transition.as_ref().filter(|transition| {
                    transition.to_index == selected_index
                        && transition.started_at.elapsed() < motion::duration::PLAYLIST_CONTENT * 2
                });
                let old =
                    transition.and_then(|transition| library.playlists.get(transition.from_index));
                (
                    old.map(|playlist| root.local_playlist_name(playlist)),
                    old.map(|playlist| playlist.tracks.len()),
                    transition
                        .map(|transition| transition.sequence)
                        .unwrap_or(0),
                )
            };
            let count_label = root.localized("{} 首", &[playlist_count.to_string()]);
            let old_count_label =
                old_count.map(|count| root.localized("{} 首", &[count.to_string()]));
            // 头图用预生成的 320px 缓存；96px 只做兜底。两者都从缓存读，
            // 不经过全尺寸封面，所以切换后不会先糊一下再变清晰。
            // 同一张图从 96px 升到 320px 只是质量升级，不能重播整段歌单切换；
            // 只有歌单身份或封面源真的变了，才走完整的交叉淡化。
            let hero_source = root.playlist_hero_source(selected_index, current);
            let hero_upgrade = matches!(
                &root.hero_shown,
                Some((shown_index, shown_source))
                    if *shown_index == selected_index && *shown_source == hero_source
            );
            let hero_path = root.playlist_hero_cover(selected_index, current);
            root.hero_cover_fade.update(hero_path);
            root.hero_shown = Some((selected_index, hero_source));
            let hero_duration = if hero_upgrade {
                motion::duration::COVER_UPGRADE
            } else {
                motion::duration::COVER
            };
            let mut actions = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::space::SM));
            if !current.tracks.is_empty() {
                actions = actions.child(
                    div()
                        .id("local-play-all")
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::space::SM))
                        .h(px(theme::size::CONTROL_LG))
                        .px(px(theme::space::XL))
                        .rounded(px(theme::radius::PILL))
                        .bg(theme::playlist_action())
                        .cursor_pointer()
                        .hover(|style| style.bg(theme::playlist_action_hover()))
                        // 按下时整颗胶囊轻微缩小，松开回弹；只作用于绘制层，不改变布局。
                        .active(|style| style.layer_scale(0.97))
                        .on_click(cx.listener(move |root, _event: &ClickEvent, _window, cx| {
                            root.play_local_playlist(selected_index, cx);
                        }))
                        .child(
                            svg()
                                .path(icons::path("play"))
                                .size(px(theme::ICON))
                                .text_color(theme::playlist_action_text()),
                        )
                        .child(
                            div()
                                .text_size(theme::Text::Body.size())
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::playlist_action_text())
                                .child(root.tr("播放全部")),
                        ),
                );
                let shuffle_fade = root.hovers.get("local-shuffle");
                actions = actions.child(
                    div()
                        .id("local-shuffle")
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(theme::size::CONTROL_LG))
                        .rounded(px(theme::radius::PILL))
                        .cursor_pointer()
                        .child(motion::hover_layer(
                            "local-shuffle",
                            shuffle_fade,
                            theme::radius::PILL,
                            theme::surface_hover(),
                        ))
                        .active(|style| style.layer_scale(0.96))
                        .on_hover(Root::hover_listener(cx, "local-shuffle"))
                        .on_click(cx.listener(move |root, _event: &ClickEvent, _window, cx| {
                            root.queue.mode = sodam_core::queue::PlayMode::Shuffle;
                            root.settings.play_mode = root.queue.mode;
                            root.persist_preferences();
                            root.play_local_playlist(selected_index, cx);
                        }))
                        .child(
                            svg()
                                .path(icons::path("shuffle"))
                                .size(px(theme::ICON))
                                .text_color(theme::text_muted()),
                        ),
                );
            }

            // 头图只做透明度交叉，不再缩放：歌单身份变化 550ms，
            // 同一张图 96→320 升级 120ms。新图没准备好时继续显示旧图。
            let hero_placeholder = div()
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
                        .size(px(48.0))
                        .text_color(theme::text_faint()),
                )
                .into_any_element();
            let mut hero_cover_element = div()
                .relative()
                .flex_none()
                .size(px(200.0))
                .rounded(px(theme::radius::CARD))
                .overflow_hidden()
                .shadow_lg();
            for layer in motion::cover_layers_scaled(
                "local-hero",
                &root.hero_cover_fade,
                hero_duration,
                theme::radius::CARD,
                hero_placeholder,
                1.0,
            ) {
                hero_cover_element = hero_cover_element.child(layer);
            }

            let hero = div()
                .w_full()
                .flex()
                .flex_row()
                .items_end()
                .gap(px(theme::space::XL))
                .child(hero_cover_element)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(theme::space::SM))
                        .min_w(px(0.0))
                        .flex_1()
                        .child(
                            div()
                                .text_size(theme::Text::Small.size())
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme::text_faint())
                                .child(root.tr("歌单")),
                        )
                        .child({
                            // 标题与高亮同时开始换：旧名从上方 2px 淡出，新名从下方 4px 进入。
                            let title_style = |text: String| -> AnyElement {
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_size(theme::Text::Hero.size())
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme::text())
                                    .child(text)
                                    .into_any_element()
                            };
                            let new = title_style(playlist_name.clone());
                            match old_name {
                                Some(from) => motion::crossfade_text(
                                    "local-title",
                                    text_seq,
                                    Some(title_style(from)),
                                    new,
                                    motion::duration::PLAYLIST_CONTENT,
                                    5.0,
                                ),
                                None => new,
                            }
                        })
                        .child({
                            let count_style = |text: String| -> AnyElement {
                                div()
                                    .text_size(theme::Text::Small.size())
                                    .text_color(theme::text_muted())
                                    .child(text)
                                    .into_any_element()
                            };
                            let new = count_style(count_label.clone());
                            match old_count_label {
                                Some(old) => motion::crossfade_text(
                                    "local-count",
                                    text_seq,
                                    Some(count_style(old)),
                                    new,
                                    motion::duration::PLAYLIST_CONTENT,
                                    4.0,
                                ),
                                None => new,
                            }
                        })
                        .child(div().h(px(theme::space::SM)))
                        .child(actions),
                );

            let playlist_index = selected_index;
            let row_count = current.tracks.len();
            let list_library = library.clone();
            let entity = cx.entity();
            let scroll_handle = root.local_scroll.clone();
            let hover_state = root.local_hover;
            let np_current = root.now_playing_fade.current.clone();
            let np_previous = root.now_playing_fade.previous.clone();
            let np_t = root.now_playing_fade.progress();
            let covers = root.cover_thumbs.clone();
            let list = uniform_list(
                ("local-track-list", playlist_index),
                row_count,
                move |range, _window, _cx| {
                    range
                        .map(|index| {
                            let entity = entity.clone();
                            let hover_entity = entity.clone();
                            let track = &list_library.playlists[playlist_index].tracks[index];
                            let path_key = track.path.to_string_lossy().to_string();
                            // 正在播放的行：序号位置在数字和音符之间交叉淡化，
                            // 换歌时旧行淡出、新行淡入，不会「啪」地换掉。
                            let strength = playing_strength(
                                np_current.as_deref(),
                                np_previous.as_deref(),
                                np_t,
                                &path_key,
                            );
                            let marker_color =
                                motion::mix_rgba(theme::text_faint(), theme::accent(), strength);
                            let cover = covers.get(&path_key).cloned().flatten();
                            div()
                                .id(gpui::ElementId::Name(
                                    format!("local-track-{playlist_index}-{index}").into(),
                                ))
                                .relative()
                                .w_full()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(theme::space::MD))
                                .h(px(theme::size::LIST_ROW))
                                .px(px(theme::space::MD))
                                .rounded(px(theme::radius::ROW))
                                .cursor_pointer()
                                .when_some(
                                    motion::row_hover_layer(
                                        "local-row",
                                        hover_state.fade_for(index),
                                        theme::radius::ROW,
                                        theme::surface_hover(),
                                    ),
                                    |this, layer| this.child(layer),
                                )
                                .on_hover(move |hovered: &bool, _window, cx: &mut gpui::App| {
                                    let next = if *hovered { Some(index) } else { None };
                                    hover_entity.update(cx, |root, cx| {
                                        let changed = match next {
                                            Some(_) => root.local_hover.set(next),
                                            None => {
                                                root.local_hover.current == Some(index)
                                                    && root.local_hover.set(None)
                                            }
                                        };
                                        if changed {
                                            cx.notify();
                                        }
                                    });
                                })
                                .on_click(
                                    move |_event: &ClickEvent, _window, cx: &mut gpui::App| {
                                        entity.update(cx, |root, cx| {
                                            root.play_local_track(playlist_index, index, cx);
                                        });
                                    },
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w(px(28.0))
                                        .h(px(18.0))
                                        .flex_none()
                                        .text_size(theme::Text::Small.size())
                                        .child(
                                            div()
                                                .absolute()
                                                .top(px(0.0))
                                                .left(px(0.0))
                                                .opacity(1.0 - strength)
                                                .text_color(theme::text_faint())
                                                .child((index + 1).to_string()),
                                        )
                                        .child(
                                            div()
                                                .absolute()
                                                .top(px(0.0))
                                                .left(px(0.0))
                                                .opacity(strength)
                                                .text_color(marker_color)
                                                .child("♪"),
                                        ),
                                )
                                .child(match cover {
                                    Some(path) => div()
                                        .flex_none()
                                        .size(px(theme::size::ROW_ART))
                                        .rounded(px(4.0))
                                        .overflow_hidden()
                                        .child(motion::thumb_img(path))
                                        .into_any_element(),
                                    None => div()
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(theme::size::ROW_ART))
                                        .rounded(px(4.0))
                                        .bg(theme::surface_elevated())
                                        .child(
                                            svg()
                                                .path(icons::path("music"))
                                                .size(px(theme::ICON_SM))
                                                .text_color(theme::text_faint()),
                                        )
                                        .into_any_element(),
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(theme::Text::Body.size())
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme::text())
                                        .child(track.title.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(200.0))
                                        .flex_none()
                                        .truncate()
                                        .text_size(theme::Text::Small.size())
                                        .text_color(theme::text_muted())
                                        .child(track.artist.clone()),
                                )
                                .child(
                                    div()
                                        .w(px(64.0))
                                        .flex_none()
                                        .text_right()
                                        .text_size(theme::Text::Small.size())
                                        .text_color(theme::text_faint())
                                        .child(local_duration(track.duration_seconds)),
                                )
                        })
                        .collect::<Vec<_>>()
                },
            )
            .size_full()
            .scrollbar_width(px(0.0))
            .track_scroll(&scroll_handle)
            .on_scroll_wheel(crate::app_scroll::uniform_wheel_listener(
                cx,
                "local-track-list",
                scroll_handle.clone(),
            ));

            let table_head = div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::space::MD))
                .h(px(36.0))
                .px(px(theme::space::MD))
                .border_b_1()
                .border_color(theme::border())
                .text_size(theme::Text::Tiny.size())
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme::text_faint())
                .child(div().w(px(28.0)).flex_none().child("#"))
                .child(div().w(px(theme::size::ROW_ART)).flex_none())
                .child(div().flex_1().min_w(px(0.0)).child(root.tr("标题")))
                .child(div().w(px(200.0)).flex_none().child(root.tr("歌手")))
                .child(
                    div()
                        .w(px(64.0))
                        .flex_none()
                        .text_right()
                        .child(root.tr("时长")),
                );

            let section_head = div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::space::SM))
                .child(
                    div()
                        .text_size(theme::Text::Large.size())
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme::text())
                        .child(root.tr("歌曲")),
                )
                .child(
                    div()
                        .text_size(theme::Text::Small.size())
                        .text_color(theme::text_faint())
                        .child(root.localized("{} 首", &[row_count.to_string()])),
                );

            column = column.child(hero).child(section_head).child(table_head);
            if row_count == 0 {
                column =
                    column.child(div().child(empty_state("music", root.tr("这个歌单里还没有歌"))));
            } else {
                column = column.child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .child(fill_remaining(list.into_any_element())),
                );
            }
        }
        _ => {
            // 没选文件夹才给「选择一个文件夹」的引导；已选但扫不出东西时，
            // 上面那行 `local_error` 已经说明原因，不再重复一段空态文案。
            if folder.is_empty() {
                column = column.child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h(px(0.0))
                        .w_full()
                        .child(empty_library_state(root, cx))
                        .with_animation(
                            "local-empty-library",
                            motion::standard(motion::duration::STATE),
                            |this, delta| this.opacity(delta),
                        ),
                );
            }
        }
    }

    column.into_any_element()
}

/// 未选文件夹（或扫描失败）时的说明 + 主按钮。
fn empty_library_state(root: &Root, cx: &mut Context<Root>) -> AnyElement {
    div()
        .w_full()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::space::LG))
        .py(px(theme::space::XXL))
        .child(
            svg()
                .path(icons::path("list-music"))
                .size(px(64.0))
                .text_color(theme::text_faint()),
        )
        .child(
            div()
                .text_size(theme::Text::Large.size())
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme::text())
                .child(root.tr("本地音乐库还是空的")),
        )
        .child(
            div()
                .max_w(px(560.0))
                .text_size(theme::Text::Small.size())
                .text_color(theme::text_muted())
                .child(root.localized(
                    "选择一个文件夹，里面的音乐会进入「全部歌单」，每个子文件夹各自成为一个歌单。",
                    &[],
                )),
        )
        .child(
            div()
                .id("local-pick-folder-empty")
                .flex()
                .items_center()
                .justify_center()
                .h(px(theme::size::CONTROL_LG))
                .px(px(theme::space::XXL))
                .rounded(px(theme::radius::PILL))
                .bg(theme::playlist_action())
                .cursor_pointer()
                .hover(|style| style.bg(theme::playlist_action_hover()))
                .text_size(theme::Text::Body.size())
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::playlist_action_text())
                .on_click(cx.listener(|root, _event: &ClickEvent, _window, cx| {
                    root.pick_local_folder(cx);
                }))
                .child(root.tr("选择文件夹")),
        )
        .into_any_element()
}

/// `mm:ss`；时长未知（0）显示 `--:--`，与在线曲目一致。
fn local_duration(seconds: u32) -> String {
    if seconds == 0 {
        "--:--".to_string()
    } else {
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }
}

/// 列表占满页面剩余高度；`uniform_list` 自己需要确定的高度。
fn fill_remaining(list: gpui::AnyElement) -> AnyElement {
    div()
        .relative()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .child(list)
        .into_any_element()
}
