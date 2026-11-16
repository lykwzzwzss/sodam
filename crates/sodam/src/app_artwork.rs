//! 封面与整页氛围：异步准备当前曲目/当前歌单的封面，切换时平滑换色。
//!
//! 取图与色板在后台线程（`artwork::prepare`），这里只负责：
//! 1. 丢弃过期结果（换歌/换歌单很快时）；
//! 2. 把封面调色板写进 `theme`，整页色场/面板一起过渡。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use gpui::{AppContext, Context};

use crate::app::Root;
use crate::artwork::ArtworkView;
use sodam_core::local::{LocalLibrary, LocalPlaylist};
use sodam_core::models::TrackItem;

/// 预热任务回传：列表缩略图或歌单头图。
enum CoverUpdate {
    Thumb(String, Option<crate::artwork::PreparedSquare>),
    Hero(usize, String, PathBuf),
}

impl Root {
    /// 整页色板的来源：优先正在播放的封面；没在播放（或这首歌没有封面）时用当前歌单封面。
    pub(crate) fn backdrop_art(&self) -> Option<&ArtworkView> {
        self.artwork.as_ref().or(self.playlist_art.as_ref())
    }

    /// 列表行/侧栏用的 96px 缩略图。
    pub(crate) fn cover_thumb_for(&self, path: &Path) -> Option<PathBuf> {
        self.cover_thumbs
            .get(&path.to_string_lossy().to_string())
            .cloned()
            .flatten()
    }

    /// 当前曲目的全尺寸封面：优先全尺寸，没准备好时退到 96px 缩略图。
    ///
    /// 播放页大封面用它；切歌时继续显示上一首，直到新封面准备好，
    /// 避免 96px 缩略图放大到 360px 先糊一下。
    pub(crate) fn now_playing_cover(&self) -> Option<PathBuf> {
        if let Some(art) = &self.artwork {
            return Some(art.cover_path.clone());
        }
        let track = self
            .pending_track
            .as_ref()
            .or_else(|| self.queue.current())?;
        let path = track.id.strip_prefix(sodam_core::local::LOCAL_ID_PREFIX)?;
        self.cover_thumb_for(Path::new(path))
    }

    /// 播放栏左下角的小封面：当前曲目一换就立刻用它的 96px 缩略图。
    ///
    /// 全尺寸封面要读标签、解码，慢的时候要等好几秒；
    /// 68px 的显示尺寸用 96px 缩略图完全够清晰，没必要等大图。
    pub(crate) fn player_bar_cover(&self) -> Option<PathBuf> {
        if let Some(track) = self.pending_track.as_ref().or_else(|| self.queue.current()) {
            if let Some(path) = track.id.strip_prefix(sodam_core::local::LOCAL_ID_PREFIX) {
                // 区分「预热过且没有封面」和「还没预热」：前者立即显示占位，
                // 不能继续挂着上一首的封面。
                match self.cover_thumbs.get(path) {
                    Some(Some(thumb)) => return Some(thumb.clone()),
                    Some(None) => return None,
                    None => {}
                }
            }
        }
        self.now_playing_cover()
    }

    /// 歌单封面：取第一首有缩略图的歌（歌单本身没有独立的封面字段）。
    pub(crate) fn playlist_cover(&self, playlist: &LocalPlaylist) -> Option<PathBuf> {
        playlist
            .tracks
            .iter()
            .find_map(|track| self.cover_thumb_for(&track.path))
    }

    /// 歌单 96px 兜底封面来自哪首源文件（用于判断同图升级）。
    fn playlist_cover_source(&self, playlist: &LocalPlaylist) -> Option<String> {
        playlist
            .tracks
            .iter()
            .find(|track| self.cover_thumb_for(&track.path).is_some())
            .map(|track| track.path.to_string_lossy().to_string())
    }

    /// 歌单头图：优先 320px 缓存（200px 显示足够清晰），没有时退回 96px。
    pub(crate) fn playlist_hero_cover(
        &self,
        index: usize,
        playlist: &LocalPlaylist,
    ) -> Option<PathBuf> {
        self.hero_thumbs
            .get(&index)
            .cloned()
            .or_else(|| self.playlist_cover(playlist))
    }

    /// 头图当前对应的源文件：320px 就绪时用它自己的源，否则用 96px 兜底的源。
    pub(crate) fn playlist_hero_source(
        &self,
        index: usize,
        playlist: &LocalPlaylist,
    ) -> Option<String> {
        if self.hero_thumbs.contains_key(&index) {
            return self.hero_sources.get(&index).cloned();
        }
        self.playlist_cover_source(playlist)
    }

    /// 切歌的瞬间先用缩略图色板启动过渡。
    ///
    /// 全尺寸封面要读标签、解码，慢的时候要等几秒；真正驱动整页视觉的是
    /// 9 阶色板。缩略图预热时已经算好了色板，这里直接拿来用，
    /// 颜色和全尺寸封面不必等同一份任务。
    pub(crate) fn preview_track_palette(&mut self, path: &Path, cx: &mut Context<Self>) {
        let key = path.to_string_lossy().to_string();
        let Some(palette) = self.cover_palettes.get(&key) else {
            // 这首歌还没预热到：不要凭空切到中性色，等全尺寸封面回来。
            return;
        };
        let target = match palette {
            Some(palette) => Some(*palette),
            // 已知没有封面：退回当前歌单封面，和整页色板的兜底一致。
            None => self.playlist_art.as_ref().map(|art| art.palette),
        };
        self.animate_palette_to(target, cx);
    }

    /// 切歌单时先用歌单里第一首已预热曲目的色板过渡（没有单曲在播时才生效）。
    pub(crate) fn preview_playlist_palette(&mut self, index: usize, cx: &mut Context<Self>) {
        // 正在播某首歌时，背景色属于这首歌，不跟着歌单列表跳。
        if self.artwork.is_some() || self.artwork_pending {
            return;
        }
        let Some(library) = self.local_library.as_ref() else {
            return;
        };
        let Some(playlist) = library.playlists.get(index) else {
            return;
        };
        // 用「头图实际来源」那张图的缓存色板，别拿歌单里随便一首歌的颜色先过渡一次；
        // 否则全尺寸封面回来时目标不同，会再播一遍主题色。
        let Some(source) = self.playlist_cover_source(playlist) else {
            return;
        };
        let palette = self.cover_palettes.get(&source).copied().flatten();
        if let Some(palette) = palette {
            self.animate_palette_to(Some(palette), cx);
        }
    }

    /// 当前歌单首屏的缩略图优先补齐；只在切歌单/扫描完成时调用，
    /// 不放在每帧渲染里（否则滚动时每帧都要拼 80 个路径）。
    pub(crate) fn request_selected_playlist_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.local_library.clone() else {
            return;
        };
        let index = self
            .local_selected
            .min(library.playlists.len().saturating_sub(1));
        let Some(playlist) = library.playlists.get(index) else {
            return;
        };
        self.request_cover_thumbs(
            playlist
                .tracks
                .iter()
                .take(80)
                .map(|track| track.path.clone())
                .collect(),
            cx,
        );
    }

    /// 为指定曲目优先补齐缩略图（搜索命中、当前歌单首屏）。
    ///
    /// 全库预热按库顺序跑，可能还没轮到用户正在看的那几首；这里插一个小批次，
    /// 每个列表只请求一次，结果照样回填 `cover_thumbs`。
    pub(crate) fn request_cover_thumbs(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let seq = self.cover_prewarm_seq;
        let mut missing = Vec::new();
        for path in paths {
            let key = path.to_string_lossy().to_string();
            if self.cover_thumbs.contains_key(&key) || !self.cover_requests.insert(key.clone()) {
                continue;
            }
            missing.push((key, path));
        }
        if missing.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel::<(String, Option<crate::artwork::PreparedSquare>)>();
        cx.background_spawn(async move {
            for (key, path) in missing {
                let thumb = crate::artwork::prepare_thumb(&path);
                if tx.send((key, thumb)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(20))
                .await;
            let mut updates = Vec::new();
            let mut disconnected = false;
            loop {
                match rx.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            let keep_running = this
                .update(cx, |root, cx| {
                    if root.cover_prewarm_seq != seq {
                        return false;
                    }
                    let changed = !updates.is_empty();
                    for (key, prepared) in updates {
                        root.cover_requests.remove(&key);
                        let (thumb, palette, cover) = match prepared {
                            Some(prepared) => (
                                Some(prepared.path),
                                Some(prepared.palette),
                                prepared.cover_path,
                            ),
                            None => (None, None, None),
                        };
                        std::sync::Arc::make_mut(&mut root.cover_thumbs).insert(key.clone(), thumb);
                        root.cover_palettes.insert(key.clone(), palette);
                        match cover {
                            Some(path) => {
                                root.cover_files.insert(key, path);
                            }
                            None => {
                                root.cover_files.remove(&key);
                            }
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                    !disconnected
                })
                .unwrap_or(false);
            if !keep_running {
                break;
            }
        })
        .detach();
    }

    /// 扫描完成后预热全库缩略图：逐首生成 96px PNG，生成一张就回 UI 一次。
    ///
    /// 397 首的整库在后台几秒内完成；列表首屏只需要等前十几张，体感比逐行懒加载稳。
    /// 同时给每个歌单准备一张 320px 头图（只取第一首有封面的歌），切歌单立即清晰。
    pub(crate) fn start_cover_prewarm(
        &mut self,
        library: Arc<LocalLibrary>,
        cx: &mut Context<Self>,
    ) {
        let mut paths: Vec<(String, PathBuf)> = Vec::new();
        let mut seen = HashSet::new();
        for playlist in &library.playlists {
            for track in &playlist.tracks {
                let key = track.path.to_string_lossy().to_string();
                if seen.insert(key.clone()) {
                    paths.push((key, track.path.clone()));
                }
            }
        }
        let hero_jobs: Vec<(usize, Vec<PathBuf>)> = library
            .playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                (
                    index,
                    playlist
                        .tracks
                        .iter()
                        .take(8)
                        .map(|track| track.path.clone())
                        .collect(),
                )
            })
            .filter(|(_, candidates): &(usize, Vec<PathBuf>)| !candidates.is_empty())
            .collect();
        let seq = self.cover_prewarm_seq.wrapping_add(1);
        self.cover_prewarm_seq = seq;
        self.cover_thumbs = Arc::new(HashMap::new());
        self.cover_palettes.clear();
        self.cover_files.clear();
        self.cover_requests.clear();
        self.hero_thumbs.clear();
        self.hero_sources.clear();
        if paths.is_empty() {
            return;
        }

        let (tx, rx) = mpsc::channel::<CoverUpdate>();
        cx.background_spawn(async move {
            // 先做每个歌单的头图：数量少（几个歌单），切歌单时最先需要。
            for (index, candidates) in hero_jobs {
                for path in candidates {
                    if let Some(hero) = crate::artwork::prepare_hero_thumb(&path) {
                        let source = path.to_string_lossy().to_string();
                        if tx.send(CoverUpdate::Hero(index, source, hero)).is_err() {
                            return;
                        }
                        break;
                    }
                }
            }
            for (index, (key, path)) in paths.into_iter().enumerate() {
                let thumb = crate::artwork::prepare_thumb(&path);
                if tx.send(CoverUpdate::Thumb(key, thumb)).is_err() {
                    break;
                }
                // 解码/编码是 CPU 密集的，偶尔让出执行器，别把扫描等任务饿死。
                if index % 8 == 0 {
                    std::thread::yield_now();
                }
            }
        })
        .detach();

        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(24))
                .await;
            let mut updates = Vec::new();
            let mut disconnected = false;
            loop {
                match rx.try_recv() {
                    Ok(update) => updates.push(update),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            let keep_running = this
                .update(cx, |root, cx| {
                    if root.cover_prewarm_seq != seq {
                        return false;
                    }
                    let changed = !updates.is_empty();
                    for update in updates {
                        match update {
                            CoverUpdate::Thumb(key, thumb) => {
                                let (path, palette, cover) = match thumb {
                                    Some(prepared) => (
                                        Some(prepared.path),
                                        Some(prepared.palette),
                                        prepared.cover_path,
                                    ),
                                    None => (None, None, None),
                                };
                                std::sync::Arc::make_mut(&mut root.cover_thumbs)
                                    .insert(key.clone(), path);
                                root.cover_palettes.insert(key.clone(), palette);
                                match cover {
                                    Some(path) => {
                                        root.cover_files.insert(key, path);
                                    }
                                    None => {
                                        root.cover_files.remove(&key);
                                    }
                                }
                            }
                            CoverUpdate::Hero(index, source, path) => {
                                root.hero_thumbs.insert(index, path);
                                root.hero_sources.insert(index, source);
                            }
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                    !disconnected
                })
                .unwrap_or(false);
            if !keep_running {
                break;
            }
        })
        .detach();
    }

    /// 为正在播放的曲目准备封面。`path` 是已经确认存在的音频文件。
    pub(crate) fn request_track_artwork(
        &mut self,
        track: &TrackItem,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let seq = self.artwork_seq.wrapping_add(1);
        self.artwork_seq = seq;
        if self
            .artwork
            .as_ref()
            .is_some_and(|art| art.track_id == track.id)
        {
            self.artwork_pending = false;
            self.apply_artwork_palette(cx);
            return;
        }
        self.artwork_pending = true;
        let track_id = track.id.clone();
        // 预热产物：原图缓存文件 + 色板。有它就不再为这一首重新解析标签。
        let key = path.to_string_lossy().to_string();
        let cached_cover = self.cover_files.get(&key).cloned();
        let cached_palette = self.cover_palettes.get(&key).copied().flatten();
        let prepared = cx.background_spawn(async move {
            crate::artwork::prepare(&track_id, &path, cached_cover.as_deref(), cached_palette)
        });
        cx.spawn(async move |this, cx| {
            let art = prepared.await;
            let _ = this.update(cx, |root, cx| {
                if root.artwork_seq != seq {
                    return;
                }
                root.set_current_artwork(art, cx);
            });
        })
        .detach();
    }

    fn set_current_artwork(&mut self, art: Option<ArtworkView>, cx: &mut Context<Self>) {
        self.artwork_pending = false;
        self.artwork = art;
        self.apply_artwork_palette(cx);
        cx.notify();
    }

    /// 为选中的歌单准备封面：取歌单里前几首有封面的歌（很多下载文件没写封面）。
    pub(crate) fn request_playlist_artwork(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(library) = self.local_library.clone() else {
            return;
        };
        let Some(playlist) = library.playlists.get(index) else {
            return;
        };
        // 单曲封面优先级更高；这里只维护兜底。
        let candidates: Vec<(
            PathBuf,
            Option<PathBuf>,
            Option<sodam_core::artwork::Palette>,
        )> = playlist
            .tracks
            .iter()
            .take(8)
            .map(|track| {
                let key = track.path.to_string_lossy().to_string();
                (
                    track.path.clone(),
                    self.cover_files.get(&key).cloned(),
                    self.cover_palettes.get(&key).copied().flatten(),
                )
            })
            .collect();
        let seq = self.playlist_art_seq.wrapping_add(1);
        self.playlist_art_seq = seq;
        if candidates.is_empty() {
            self.set_playlist_artwork(None, cx);
            return;
        }
        let prepared = cx.background_spawn(async move {
            for (path, cover, palette) in candidates {
                if let Some(art) =
                    crate::artwork::prepare("playlist", &path, cover.as_deref(), palette)
                {
                    return Some(art);
                }
            }
            None
        });
        cx.spawn(async move |this, cx| {
            let art = prepared.await;
            let _ = this.update(cx, |root, cx| {
                if root.playlist_art_seq != seq || root.local_selected != index {
                    return;
                }
                root.set_playlist_artwork(art, cx);
            });
        })
        .detach();
    }

    fn set_playlist_artwork(&mut self, art: Option<ArtworkView>, cx: &mut Context<Self>) {
        self.playlist_art = art;
        if self.artwork.is_none() && !self.artwork_pending {
            self.apply_artwork_palette(cx);
        }
        cx.notify();
    }

    /// 按「正在播放优先、否则当前歌单」的封面色板，平滑切换整页颜色。
    pub(crate) fn apply_artwork_palette(&mut self, cx: &mut Context<Self>) {
        let palette = self.backdrop_art().map(|art| art.palette);
        self.animate_palette_to(palette, cx);
    }

    /// 扫描完成后默认选中「全部歌单」，顺手准备它的封面。
    pub(crate) fn refresh_selected_playlist_art(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.local_library.clone() else {
            return;
        };
        let index = self
            .local_selected
            .min(library.playlists.len().saturating_sub(1));
        self.request_playlist_artwork(index, cx);
    }
}
