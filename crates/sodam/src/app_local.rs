//! 本地音乐库：选文件夹、后台扫描、离线播放。
//!
//! 契约见 `docs/LOCAL_LIBRARY.md` 契约 D。这里只做三件事：把文件夹写进设置、
//! 在后台线程扫描成 [`sodam_core::local::LocalLibrary`]、把歌单交给队列播放。
//! 播放走 `play_from` → `start_track` → `start_local_track` 的既有链路：
//! 本地曲目直接 `engine.load`：不联网，不碰下载缓存与评测体系。
//! 预取和签名服务。

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{AppContext, Context};
use sodam_core::local::{self, LocalPlaylist, LocalPlaylistKind};
use sodam_core::log_outlet::{log_event, LogLevel, LogTopic};
use sodam_core::models::TrackItem;

use crate::app::{Root, SAVE_TEXT};

impl Root {
    /// 系统文件夹选择框：选中后写入设置、保存、重新扫描。
    ///
    /// 对话框是阻塞式的，必须放在后台执行器里；UI 线程只等结果。
    pub(crate) fn pick_local_folder(&mut self, cx: &mut Context<Self>) {
        // 只有 Windows 接了系统文件夹选择框；其它平台给出可行动的说明，
        // 而不是让按钮点了完全没反应。
        if !folder_picker_supported() {
            let message = self
                .tr("这个平台暂不支持选择文件夹，请在 config.json 里填写 local_music_folder")
                .to_string();
            self.set_status_text(message);
            cx.notify();
            return;
        }
        let start = {
            let folder = self.settings.local_music_folder.trim();
            if folder.is_empty() {
                None
            } else {
                Some(PathBuf::from(folder))
            }
        };
        let title = self.tr("本地音乐文件夹").to_string();
        let picked = cx.background_spawn(async move { pick_folder_blocking(start, title) });
        cx.spawn(async move |this, cx| {
            let Some(folder) = picked.await else {
                return;
            };
            let folder = folder.to_string_lossy().to_string();
            let _ = this.update(cx, |root, cx| {
                root.settings.local_music_folder = folder;
                if let Err(error) = root.settings.save() {
                    let message = root.failure_text("保存本地音乐文件夹失败", &error, SAVE_TEXT);
                    // 紧跟着就要显示「正在扫描…」，状态行会被覆盖；失败改走轻提示。
                    root.notify_error_once("local-folder-save-failed", message);
                }
                root.local_selected = 0;
                root.local_error = None;
                root.scan_local_library(cx);
            });
        })
        .detach();
    }

    /// 按配置里的文件夹在后台扫描，回 UI 线程整体替换扫描结果。
    pub(crate) fn scan_local_library(&mut self, cx: &mut Context<Self>) {
        self.local_scan_generation = self.local_scan_generation.wrapping_add(1);
        let generation = self.local_scan_generation;
        let target = self.settings.local_music_folder.trim().to_string();
        if target.is_empty() {
            self.local_library = None;
            self.local_loading = false;
            self.local_loading_visible = false;
            self.local_error = None;
            self.cover_prewarm_seq = self.cover_prewarm_seq.wrapping_add(1);
            self.cover_thumbs = std::sync::Arc::new(std::collections::HashMap::new());
            self.cover_palettes.clear();
            self.cover_files.clear();
            self.cover_requests.clear();
            self.search_cache = None;
            self.local_selected = 0;
            self.sidebar_selection_shown = 0;
            self.sidebar_marker_from = 0;
            self.sidebar_marker_seq = 0;
            self.sidebar_rows.clear();
            cx.notify();
            return;
        }

        self.local_loading = true;
        self.local_loading_visible = false;
        self.local_error = None;
        self.search_cache = None;
        self.set_status("正在扫描本地音乐…", &[]);
        // 扫描常常几十毫秒就结束；加载指示延后 180ms 出现，
        // 避免快速任务先闪一下 spinner 再消失。
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::ui::motion::duration::STATE)
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.local_scan_generation == generation
                    && root.local_loading
                    && !root.local_loading_visible
                {
                    root.local_loading_visible = true;
                    cx.notify();
                }
            });
        })
        .detach();

        let scan_path = PathBuf::from(&target);
        let scanned = cx.background_spawn(async move { local::scan(&scan_path) });
        cx.spawn(async move |this, cx| {
            let library = scanned.await;
            let _ = this.update(cx, |root, cx| {
                // 更新的扫描已启动：这批结果作废。
                if root.local_scan_generation != generation {
                    return;
                }
                root.local_loading = false;
                root.local_loading_visible = false;
                let playlists = library.playlists.len();
                let track_count = library.track_count();
                let selected_dir = root
                    .local_library
                    .as_ref()
                    .and_then(|previous| previous.playlists.get(root.local_selected))
                    .map(|playlist| playlist.dir.as_path());
                root.local_selected = playlist_index(&library, selected_dir);
                // 重新扫描后行状态整体重建，旧的悬停/选中动画不能沿用到新列表。
                root.sidebar_selection_shown = root.local_selected;
                root.sidebar_marker_from = root.local_selected;
                root.sidebar_marker_seq = 0;
                root.sidebar_rows.clear();
                root.local_hover.clear();
                root.playlist_transition = None;
                if playlists == 0 {
                    // 新文件夹读不到：清掉旧库，避免继续展示上一个文件夹的曲目。
                    root.local_library = None;
                    root.search_cache = None;
                    let mut message = root.tr("读取本地音乐失败：").to_string();
                    message.push_str(&target);
                    root.local_error = Some(message);
                } else {
                    root.local_error = None;
                    let library = std::sync::Arc::new(library);
                    root.local_library = Some(library.clone());
                    if root.settings.loudness_equalization {
                        if let Some(all) = library.playlists.first() {
                            let items = all
                                .tracks
                                .iter()
                                .map(|t| (format!("local:{}", t.path.display()), t.path.clone()))
                                .collect();
                            root.engine.loudness().enqueue_missing(items);
                        }
                    }
                    root.set_status("{} 首", &[track_count.to_string()]);
                    // 侧栏选中项可能已变化；封面兜底跟着当前歌单走。
                    root.refresh_selected_playlist_art(cx);
                    // 全库缩略图在后台预热，列表行/侧栏随后逐张出现。
                    root.start_cover_prewarm(library.clone(), cx);
                    // 侧栏每个歌单先给几首优先级，首屏很快能看到封面。
                    let priority: Vec<std::path::PathBuf> = library
                        .playlists
                        .iter()
                        .flat_map(|playlist| {
                            playlist
                                .tracks
                                .iter()
                                .take(4)
                                .map(|track| track.path.clone())
                        })
                        .collect();
                    root.request_cover_thumbs(priority, cx);
                    root.request_selected_playlist_thumbs(cx);
                    root.restore_session(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 恢复上次会话：按保存的队列重建播放列表，定位到上次的曲目与进度，保持暂停。
    ///
    /// 只尝试一次。保存队列里文件已经不在的曲目直接跳过；整条队列都不在了就原样
    /// 保留会话文件，等曲库恢复（移动硬盘重新插上）后下次启动还能接着恢复。
    fn restore_session(&mut self, cx: &mut Context<Self>) {
        if !self.session_pending {
            return;
        }
        let Some(library) = self.local_library.clone() else {
            return;
        };
        self.session_pending = false;
        let Some(all) = library.playlists.get(library.all_index()) else {
            return;
        };
        let available: HashMap<String, TrackItem> = all
            .tracks
            .iter()
            .map(|track| track.to_item())
            .map(|item| (item.id.clone(), item))
            .collect();
        // 旧版本只在配置里记了单曲与进度：升级后按单曲队列恢复一次。
        let legacy_id = self.settings.last_track_id.trim().to_string();
        let is_legacy = self.saved_session.is_none() && !legacy_id.is_empty();
        let session = match self.saved_session.take() {
            Some(session) => session,
            None if !legacy_id.is_empty() => sodam_core::session::PlaybackSession {
                queue: vec![legacy_id.clone()],
                ..Default::default()
            },
            None => return,
        };
        let checkpoint = if is_legacy {
            Some(sodam_core::session::PlaybackCheckpoint {
                track_id: legacy_id.clone(),
                position_ms: self.settings.last_position_ms,
                ..Default::default()
            })
        } else {
            self.saved_checkpoint.take()
        };
        // 浏览的歌单和播放队列分开恢复；文件夹改名/移走认不出时回「全部歌单」。
        // 先于队列解析：队列里的文件全都不在了，浏览位置也照样恢复。
        let selected = (!session.selected_playlist.is_empty())
            .then(|| std::path::Path::new(&session.selected_playlist));
        let index = playlist_index(&library, selected);
        if index != self.local_selected {
            self.local_selected = index;
            self.sidebar_selection_shown = index;
            self.sidebar_marker_from = index;
            self.refresh_selected_playlist_art(cx);
            self.request_selected_playlist_thumbs(cx);
        }
        let Some(resolved) = session.resolve(&available, checkpoint.as_ref()) else {
            self.saved_session = Some(session);
            return;
        };

        self.playback_generation = session.playback_generation;
        // 恢复结果记一行：用户反馈「恢复不对」时，先看这里就能区分是解析、
        // 曲库匹配还是进度规则把它归零了。
        {
            let index = resolved.state.index.to_string();
            let position = resolved.position_ms.to_string();
            let tracks = resolved.tracks.len().to_string();
            log_event(
                LogLevel::Info,
                LogTopic::App,
                "session_restored",
                &[
                    ("index", index.as_str()),
                    ("position_ms", position.as_str()),
                    ("tracks", tracks.as_str()),
                ],
            );
        }
        self.queue.restore(resolved.tracks, resolved.state);
        self.track_swap_dir = 0.0;
        self.sync_queue_cache();
        self.resume_at = Some(resolved.position_ms as f64 / 1000.0);
        self.start_track(cx);
    }

    /// 选中歌单（只换右侧曲目列表，不触发播放）。
    pub(crate) fn select_local_playlist(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(library) = self.local_library.as_ref() else {
            return;
        };
        if index >= library.playlists.len() {
            return;
        }
        let was_settings = self.settings_open;
        self.settings_open = false;
        if index == self.local_selected {
            if was_settings {
                cx.notify();
            }
            return;
        }
        // 一次切换只记一个时间基准；标题、头图仍从这里取序号和起点。
        let from_index = self.local_selected;
        self.local_selected = index;
        let sequence = self.playlist_transition_seq.wrapping_add(1);
        self.playlist_transition_seq = sequence;
        self.playlist_transition = Some(crate::app::PlaylistTransition {
            from_index,
            to_index: index,
            started_at: std::time::Instant::now(),
            sequence,
        });
        // 列表直接换内容，新歌单从顶部开始。
        self.local_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
        self.sync_scroll_anims();
        // 行号对应的曲目已经变了，旧的悬停行不能再保持高亮。
        self.local_hover.clear();
        self.close_queue(cx);
        // 没在播放时，整窗背景跟着歌单封面走。先用已预热的色板立刻过渡，
        // 全尺寸封面准备好后再校正一次。
        self.preview_playlist_palette(index, cx);
        self.request_playlist_artwork(index, cx);
        self.request_selected_playlist_thumbs(cx);
        self.persist_session();
        cx.notify();
    }

    /// 「播放全部」：整个歌单入队，从第一首开始。
    pub(crate) fn play_local_playlist(&mut self, playlist: usize, cx: &mut Context<Self>) {
        let Some(library) = self.local_library.clone() else {
            return;
        };
        let Some(entry) = library.playlists.get(playlist) else {
            return;
        };
        if entry.tracks.is_empty() {
            self.set_status("这个歌单里还没有歌", &[]);
            cx.notify();
            return;
        }
        let tracks: Vec<TrackItem> = entry.tracks.iter().map(|track| track.to_item()).collect();
        self.local_selected = playlist;
        let start = if self.queue.mode == sodam_core::queue::PlayMode::Shuffle {
            sodam_core::queue::random_index(tracks.len())
        } else {
            0
        };
        // 入队 + 起播统一交给 play_from：本地曲目在 start_track 里按 id 前缀分叉，
        // 直接 engine.load，不经过下载/预取管线。
        self.play_from(tracks, start, cx);
    }

    /// 播放歌单里的某一首：整个歌单入队，从这一首开始。
    pub(crate) fn play_local_track(
        &mut self,
        playlist: usize,
        track: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(library) = self.local_library.clone() else {
            return;
        };
        let Some(entry) = library.playlists.get(playlist) else {
            return;
        };
        if track >= entry.tracks.len() {
            self.set_status("这个歌单里还没有歌", &[]);
            cx.notify();
            return;
        }
        let tracks: Vec<TrackItem> = entry.tracks.iter().map(|item| item.to_item()).collect();
        self.local_selected = playlist;
        self.play_from(tracks, track, cx);
    }

    /// 歌单显示名：core 不认识中文，文案集中在这里。
    pub(crate) fn local_playlist_name(&self, playlist: &LocalPlaylist) -> String {
        match playlist.kind {
            LocalPlaylistKind::All => self.tr("全部歌单").to_string(),
            LocalPlaylistKind::Folder => {
                if playlist.name.trim().is_empty() {
                    playlist
                        .dir
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default()
                } else {
                    playlist.name.clone()
                }
            }
        }
    }

    /// 本地搜索：跨所有歌单去重，匹配标题/歌手/专辑（大小写不敏感）。
    ///
    /// 结果保持「全部歌单」的原始顺序（根目录文件 → 子文件夹），用户按文件夹找歌的
    /// 记忆不会被搜索结果打散。结果按「查询词 + 曲库」缓存：播放中每帧重绘
    /// 不会反复遍历全库、反复做 `to_lowercase()`。
    pub(crate) fn search_results(&mut self) -> std::sync::Arc<Vec<sodam_core::local::LocalTrack>> {
        let query = self.search_field.text().trim().to_lowercase();
        if query.is_empty() {
            return std::sync::Arc::new(Vec::new());
        }
        let Some(library) = self.local_library.as_ref() else {
            return std::sync::Arc::new(Vec::new());
        };
        let library_key = std::sync::Arc::as_ptr(library) as usize;
        if let Some((cached_query, cached_key, results)) = &self.search_cache {
            if *cached_key == library_key && *cached_query == query {
                return results.clone();
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut results = Vec::new();
        for playlist in &library.playlists {
            for track in &playlist.tracks {
                let key = track.path.to_string_lossy().to_string();
                if !seen.insert(key) {
                    continue;
                }
                if track.title.to_lowercase().contains(&query)
                    || track.artist.to_lowercase().contains(&query)
                    || track.album.to_lowercase().contains(&query)
                {
                    results.push(track.clone());
                }
            }
        }
        let results = std::sync::Arc::new(results);
        self.search_cache = Some((query, library_key, results.clone()));
        results
    }

    /// 播放搜索结果里的第 `index` 首（整个结果集入队）。
    pub(crate) fn play_search_track(&mut self, index: usize, cx: &mut Context<Self>) {
        let results = self.search_results();
        if results.is_empty() || index >= results.len() {
            return;
        }
        let tracks: Vec<TrackItem> = results.iter().map(|track| track.to_item()).collect();
        self.play_from(tracks, index, cx);
    }
}

fn playlist_index(library: &local::LocalLibrary, dir: Option<&std::path::Path>) -> usize {
    dir.and_then(|dir| {
        library
            .playlists
            .iter()
            .position(|playlist| playlist.dir == dir)
    })
    .unwrap_or_else(|| library.all_index())
}

#[cfg(target_os = "windows")]
fn pick_folder_blocking(start: Option<PathBuf>, title: String) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(directory) = start {
        dialog = dialog.set_directory(directory);
    }
    dialog.pick_folder()
}

#[cfg(target_os = "windows")]
fn folder_picker_supported() -> bool {
    true
}

#[cfg(not(target_os = "windows"))]
fn folder_picker_supported() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
fn pick_folder_blocking(_start: Option<PathBuf>, _title: String) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn playlist_selection_survives_a_folder_inserted_before_it() {
        let playlists = ["music", "music/a", "music/b"]
            .iter()
            .enumerate()
            .map(|(index, dir)| LocalPlaylist {
                kind: if index == 0 {
                    LocalPlaylistKind::All
                } else {
                    LocalPlaylistKind::Folder
                },
                name: String::new(),
                dir: PathBuf::from(dir),
                tracks: Vec::new(),
            })
            .collect();
        let library = local::LocalLibrary { playlists };
        assert_eq!(playlist_index(&library, Some(Path::new("music/b"))), 2);
        assert_eq!(playlist_index(&library, Some(Path::new("music/gone"))), 0);
    }
}
