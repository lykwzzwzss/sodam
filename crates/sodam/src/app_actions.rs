//! 用户动作层：本地音乐库、播放、设置。纯本地：不联网、不登录。
//!
//! [`Root`] 的字段定义在 [`super`]，这里按领域拆分实现，避免单文件继续膨胀。

use gpui::prelude::*;
use gpui::Context;
use sodam_core::local::LOCAL_ID_PREFIX;
use sodam_core::log_outlet::{self, log_event, LogLevel, LogTopic};
use sodam_core::models::TrackItem;
use std::sync::Arc;

use super::{Root, SAVE_TEXT};

impl Root {
    /// 展开队列：换一个动画代次，面板从右侧滑入。
    pub(crate) fn open_queue(&mut self, cx: &mut Context<Self>) {
        self.queue_anim_seq = self.queue_anim_seq.wrapping_add(1);
        self.queue_anim_at = Some(std::time::Instant::now());
        self.queue_closing = false;
        self.queue_open = true;
        self.queue_scroll.scroll_to_item(
            crate::ui::player_bar::now_playing_slot(self),
            gpui::ScrollStrategy::Center,
        );
        // 抽屉里每一行都要封面。全库预热通常已经补齐，这里只兜住旧队列
        // 或预热还没轮到的曲目；已在 `cover_requests` 里的会自动跳过。
        self.sync_queue_cache();
        let paths: Vec<std::path::PathBuf> = self
            .queue_cache
            .1
            .iter()
            .filter_map(|track| {
                track
                    .id
                    .strip_prefix(sodam_core::local::LOCAL_ID_PREFIX)
                    .map(std::path::PathBuf::from)
            })
            .collect();
        self.request_cover_thumbs(paths, cx);
        cx.notify();
    }

    /// 关闭队列：先保留面板播放滑出动画，200ms 后再真正卸载。
    ///
    /// 若关闭途中又打开，代次变化会让旧定时器失效，面板从当前位置重新滑入。
    pub(crate) fn close_queue(&mut self, cx: &mut Context<Self>) {
        if !self.queue_open && !self.queue_closing {
            return;
        }
        self.queue_open = false;
        self.queue_closing = true;
        self.queue_menu = None;
        let seq = self.queue_anim_seq.wrapping_add(1);
        self.queue_anim_seq = seq;
        self.queue_anim_at = Some(std::time::Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            // 等滑出动画走完再卸载面板；少等一帧都会在末尾露出一次底色。
            cx.background_executor()
                .timer(
                    crate::ui::motion::duration::DRAWER_CLOSE
                        + std::time::Duration::from_millis(20),
                )
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.queue_anim_seq != seq {
                    return;
                }
                root.queue_closing = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        if self.queue_open || self.queue_closing {
            self.close_queue(cx);
        } else {
            self.open_queue(cx);
        }
    }

    /// 从队列移除某一首（右键菜单）。正在播放的那首不允许移除：
    /// 它还挂在引擎上，移除后「队列当前项」与「实际在播」会分叉。
    pub fn remove_from_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.queue.index() {
            self.close_queue_menu(cx);
            let message = self
                .tr("正在播放的曲目不能从队列移除，可直接点「下一首」")
                .to_string();
            self.notify_warning_once("remove-current-track", message);
            cx.notify();
            return;
        }
        // 删除前记下槽位和滚动位置：幽灵行要停在原处淡出，
        // 下面的行同时上移归位（数据已经先删掉，动画不阻塞业务状态）。
        let slot = crate::ui::player_bar::slot_of_track(self, index);
        let track = self.queue.tracks().get(index).cloned();
        let scroll_top = f32::from(self.queue_scroll.0.borrow().base_handle.offset().y);
        if self.queue.remove(index).is_some() {
            self.close_queue_menu(cx);
            if let (Some(slot), Some(track)) = (slot, track) {
                let epoch = self.queue_reflow_seq.wrapping_add(1);
                self.queue_reflow_seq = epoch;
                self.queue_removal = Some(crate::app::QueueRemoval {
                    epoch,
                    slot,
                    track,
                    scroll_top,
                    at: std::time::Instant::now(),
                });
                // 退出 + 重排都结束后收走幽灵层。
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(240))
                        .await;
                    let _ = this.update(cx, |root, cx| {
                        if root
                            .queue_removal
                            .as_ref()
                            .is_some_and(|removal| removal.epoch == epoch)
                        {
                            root.queue_removal = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            self.sync_queue_cache();
            self.persist_session();
            // 移除当前项之前的曲目会让下标整体左移一位：进度文件里记的下标
            // 也要立刻跟着改，否则崩溃后恢复时下标指向了别的曲目。
            let snapshot = self.engine.snapshot();
            self.persist_checkpoint(&snapshot);
            cx.notify();
        }
    }

    /// 打开队列右键菜单：换一个动画代次，菜单从点击位置轻微放大进入。
    pub(crate) fn open_queue_menu(
        &mut self,
        index: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.queue_menu_seq = self.queue_menu_seq.wrapping_add(1);
        self.queue_menu_closing = false;
        self.queue_menu = Some(crate::app::QueueMenu { index, position });
        cx.notify();
    }

    /// 收起队列右键菜单：先播放 90ms 的淡出/回缩，再真正卸载。
    pub(crate) fn close_queue_menu(&mut self, cx: &mut Context<Self>) {
        if self.queue_menu.is_none() || self.queue_menu_closing {
            return;
        }
        self.queue_menu_closing = true;
        let seq = self.queue_menu_seq;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(crate::ui::motion::duration::MENU_OUT)
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.queue_menu_closing && root.queue_menu_seq == seq {
                    root.queue_menu = None;
                    root.queue_menu_closing = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 队列变化后刷新快照（抽屉渲染只读它，不再每帧 to_vec）。
    pub(crate) fn sync_queue_cache(&mut self) {
        let revision = self.queue.revision();
        if self.queue_cache.0 != revision {
            self.queue_cache = (revision, Arc::new(self.queue.tracks().to_vec()));
            // 队列行号与曲目的对应关系变了，旧的悬停行必须重新计算。
            self.queue_hover.clear();
        }
    }

    /// 按比例跳转播放位置（0.0~1.0）。
    pub fn seek_fraction(&mut self, fraction: f32) {
        let snapshot = self.engine.snapshot();
        if snapshot.duration_seconds <= 0.0 {
            return;
        }
        let target = snapshot.duration_seconds * fraction.clamp(0.0, 1.0) as f64;
        self.engine.seek(target);
        self.progress_anchor_at = std::time::Instant::now();
        self.progress_anchor_seconds = target;
        self.progress_anchor_track = snapshot.track_id.clone();
        let snapshot = self.engine.snapshot();
        self.persist_checkpoint(&snapshot);
    }

    /// 歌词行点击跳转：毫秒 → 秒 → 现有 seek 通道。
    pub(crate) fn seek_to_ms(&mut self, at_ms: u64) {
        let snapshot = self.engine.snapshot();
        if snapshot.duration_seconds <= 0.0 {
            return;
        }
        let target = at_ms as f64 / 1000.0;
        self.engine
            .seek(target.min(snapshot.duration_seconds.max(0.0)));
        self.progress_anchor_at = std::time::Instant::now();
        self.progress_anchor_seconds = target.min(snapshot.duration_seconds.max(0.0));
        self.progress_anchor_track = snapshot.track_id.clone();
        let snapshot = self.engine.snapshot();
        self.persist_checkpoint(&snapshot);
    }

    /// 设置音量（0.0~1.0）。
    pub fn set_volume(&mut self, volume: f32) {
        self.settings.set_volume(volume);
        self.engine.set_volume(self.settings.volume);
        self.persist_playback_preferences();
    }

    /// 静音 / 恢复。
    pub fn toggle_mute(&mut self) {
        self.settings.toggle_mute();
        self.engine.set_volume(self.settings.volume);
        self.persist_playback_preferences();
    }

    fn persist_playback_preferences(&mut self) {
        self.preferences_writer.schedule(&self.settings);
    }

    /// 把当前曲目与进度交给后台单写者（暂停、seek、切歌、退出时立即调用）。
    ///
    /// 位置用「快照 + 锚点外推」后的显示值：音频线程的快照最多落后 100ms，
    /// 直接存它会让每次退出都少记一小截。
    pub(crate) fn persist_checkpoint(&mut self, snapshot: &sodam_core::PlaybackSnapshot) {
        // 装载中即使 id 相同，快照也可能仍来自上一次播放。
        if self.checkpoint_load_seq != Some(snapshot.load_seq) {
            return;
        }
        let Some(track) = self.queue.current() else {
            return;
        };
        if track.id.is_empty() || track.id != snapshot.track_id {
            return;
        }
        let position = if snapshot.finished {
            0.0
        } else {
            self.position_for_snapshot(snapshot)
        };
        let checkpoint = sodam_core::session::PlaybackCheckpoint {
            track_id: track.id.clone(),
            index: self.queue.index(),
            playback_generation: self.playback_generation,
            position_ms: (position.max(0.0) * 1000.0).round() as u64,
            ..Default::default()
        };
        self.checkpoint_track = checkpoint.track_id.clone();
        self.checkpoint_playing = snapshot.playing;
        self.checkpoint_at = std::time::Instant::now();
        self.session_writer.schedule_checkpoint(checkpoint);
    }

    /// 把整条队列、当前项与浏览中的歌单交给后台单写者（换队、移除、切歌单、退出）。
    ///
    pub(crate) fn persist_session(&mut self) {
        if self.queue.is_empty() {
            // 仅浏览时也保存选中项。曲库暂时离线时保留上次的播放队列。
            if self.local_library.is_some() && !self.session_pending {
                let mut session = self.saved_session.clone().unwrap_or_default();
                session.selected_playlist = self.selected_playlist_dir();
                self.session_writer.schedule_session(session);
            }
            return;
        }
        let session = sodam_core::session::PlaybackSession {
            queue: self
                .queue
                .tracks()
                .iter()
                .map(|track| track.id.clone())
                .collect(),
            state: self.queue.state(),
            playback_generation: self.playback_generation,
            selected_playlist: self.selected_playlist_dir(),
            ..Default::default()
        };
        self.session_writer.schedule_session(session);
    }

    /// 当前浏览歌单的目录；「全部歌单」用空串（目录会随文件夹移动，认不出就回全部歌单）。
    fn selected_playlist_dir(&self) -> String {
        self.local_library
            .as_ref()
            .and_then(|library| library.playlists.get(self.local_selected))
            .filter(|playlist| playlist.kind == sodam_core::local::LocalPlaylistKind::Folder)
            .map(|playlist| playlist.dir.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 给视图层用的显式落盘入口（例如本地页的「随机播放」按钮）。
    pub(crate) fn persist_preferences(&mut self) {
        self.persist_playback_preferences();
    }

    /// 跳到队列中的某一首并播放。
    pub fn jump_in_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.queue.tracks().get(index).is_none() {
            return;
        }
        if log_outlet::enabled(LogLevel::Debug, LogTopic::Player) {
            let index = index.to_string();
            log_event(
                LogLevel::Debug,
                LogTopic::Player,
                "queue_jump",
                &[("index", index.as_str())],
            );
        }
        if self.queue.jump(index).is_some() {
            self.track_swap_dir = 0.0;
            self.start_track(cx);
        }
    }

    /// 把整张列表入队并从 `index` 开始播放（「播放全部」与开发入口）。
    ///
    /// 换队时保留用户选好的播放模式：这里不强制重置成顺序播放，
    /// 否则每次点歌都丢掉「单曲循环 / 随机播放」。
    pub fn play_from(&mut self, tracks: Vec<TrackItem>, index: usize, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        self.queue.replace(tracks, index);
        self.track_swap_dir = 0.0;
        // 列表整体换队时清掉旧播放历史，避免抽屉把上一批队列误标成“已播放”。
        self.played_history.clear();
        self.queue.prepare_shuffle_round();
        self.sync_queue_cache();
        self.start_track(cx);
    }

    /// 播放 / 暂停：引擎没装载当前曲目时先装载，否则只切换播放状态。
    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.pending_track.is_some() {
            return;
        }
        let snapshot = self.engine.snapshot();
        let needs_load = snapshot.track_id.is_empty()
            || !snapshot.error.is_empty()
            || snapshot.finished
            || self
                .queue
                .current()
                .is_some_and(|track| track.id != snapshot.track_id);
        if needs_load {
            if self.queue.is_empty() {
                self.set_status_text(self.tr("队列是空的：在本地音乐页点一首歌开始播放"));
                cx.notify();
                return;
            }
            // 队列里有歌但引擎还没装载 → 装载当前曲目
            self.start_track(cx);
            return;
        }
        self.engine.toggle();
        let playing = !snapshot.playing;
        let message = if playing {
            self.tr("播放中").to_string()
        } else {
            self.tr("已暂停").to_string()
        };
        self.set_status_text(message);
        cx.notify();
    }

    /// 装载并播放「当前队列曲目」。本地文件装载没有重试预算：失败给出原因，
    /// 由用户决定重试或切歌。
    pub fn start_track(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.queue.current().cloned() else {
            self.set_status_text(self.tr("队列是空的：在本地音乐页点一首歌开始播放"));
            cx.notify();
            return;
        };
        self.play_seq = self.play_seq.wrapping_add(1);
        self.checkpoint_track.clear();
        self.checkpoint_load_seq = None;
        if self.resume_at.is_none() {
            self.playback_generation = self.playback_generation.wrapping_add(1).max(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            );
            self.persist_session();
        }
        self.pending_track = Some(track.clone());
        self.pending_engine_load = None;
        self.progress_preview = None;
        if self.resume_at.is_none() {
            self.set_status("正在准备播放：{}…", std::slice::from_ref(&track.title));
        }
        cx.notify();

        // 本地曲目：文件就在本机，没有取流 / 解密 / 落缓存这条链路。
        // 点歌、下一首、自动续播、单曲循环最后都汇到 start_track。
        let Some(path) = track
            .id
            .strip_prefix(LOCAL_ID_PREFIX)
            .map(std::path::PathBuf::from)
        else {
            // 不是本地 id（旧序列化数据、或调用方传错）：不猜怎么播，给出原因。
            self.pending_track = None;
            self.resume_at = None;
            let message = self
                .tr("这条曲目不是本地文件，重新扫描本地音乐库后即可播放")
                .to_string();
            self.notify_warning_once("stale-track-in-queue", message);
            cx.notify();
            return;
        };
        self.start_local_track(track, path, cx);
    }

    /// 本地曲目的装载：**不联网**，只是把磁盘上的文件交给播放引擎。
    ///
    /// 与「序号 + pending」约定一致：`play_seq` 在调用方已经自增过，
    /// 这里回来时若已被更新的播放请求取代就整个作废；装载结果靠
    /// `pending_engine_load == snapshot.load_seq` 在心跳里收尾（清 loading 态），
    /// 因此这里只登记序号，不自己判断「装好了没有」。
    ///
    /// 文件存在性检查是一次文件操作，放后台线程：渲染线程上不该有 IO。
    fn start_local_track(
        &mut self,
        track: TrackItem,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let request_seq = self.play_seq;
        let resume = self.resume_at.take();
        let path_for_check = path.clone();
        let work = cx.background_spawn(async move { path_for_check.exists() });
        cx.spawn(async move |this, cx| {
            let exists = work.await;
            let _ = this.update(cx, |root, cx| {
                if root.play_seq != request_seq {
                    return;
                }
                if !exists {
                    // 文件在扫描之后被移走/删除：给出能行动的原因，不自动跳歌。
                    root.pending_track = None;
                    let message = root
                        .tr("这个文件已经不在原来的位置了，请重新扫描后播放")
                        .to_string();
                    root.notify_error_once("local-file-missing", message);
                    cx.notify();
                    return;
                }
                // 先让背景色/播放栏小封面立刻朝这首歌走：缩略图预热时已经
                // 算好色板、落好 96px 图，不必等全尺寸封面解码完。
                if root.cover_thumb_for(&path).is_none() {
                    root.request_cover_thumbs(vec![path.clone()], cx);
                }
                root.preview_track_palette(&path, cx);
                let sequence = match resume {
                    Some(seconds) => {
                        root.engine
                            .load_at(track.clone(), path.clone(), seconds, true)
                    }
                    None => root.engine.load(track.clone(), path.clone()),
                };
                root.reset_progress_anchor();
                // 先退回歌单封面，等这首的封面在后台准备好再淡入。
                // 旧封面继续显示，直到新封面的异步结果回来（序号过期即丢弃）。
                root.artwork_seq = root.artwork_seq.wrapping_add(1);
                root.request_lyrics(&track, path.clone(), cx);
                root.request_track_artwork(&track, path, cx);
                root.pending_engine_load = Some(sequence);
                root.playback_error = None;
                if root.played_history.last().map(String::as_str) != Some(track.id.as_str()) {
                    root.played_history.push(track.id.clone());
                    let overflow = root.played_history.len().saturating_sub(100);
                    if overflow > 0 {
                        root.played_history.drain(0..overflow);
                    }
                }
                if resume.is_some() {
                    root.set_status("已恢复上次播放：{}", std::slice::from_ref(&track.title));
                } else {
                    root.set_status("播放中：{}", std::slice::from_ref(&track.title));
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Root {
    /// UI 心跳：播放中每 200ms 刷新一次，
    /// 曲目播完则自动切下一首。
    pub(crate) fn start_heartbeat(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(200))
                .await;
            let _ = this.update(cx, |root, cx| {
                let snap = root.engine.snapshot();
                root.observe_progress(&snap);
                // 加载完成：先清掉 pending，让新曲目的进度能在这一拍保存。
                if root.pending_engine_load == Some(snap.load_seq) {
                    root.pending_engine_load = None;
                    root.pending_track = None;
                    if snap.error.is_empty() {
                        root.checkpoint_load_seq = Some(snap.load_seq);
                    }
                    root.persist_session();
                    cx.notify();
                }
                // 播放进度：切歌、暂停/继续时立刻落盘，播放中每 5 秒记一次。
                // 暂停后不再周期性写盘，避免长时间挂着还反复改文件。
                let now = std::time::Instant::now();
                let checkpoint_due = root.checkpoint_load_seq == Some(snap.load_seq)
                    && !snap.track_id.is_empty()
                    && (root.checkpoint_track != snap.track_id
                        || snap.playing != root.checkpoint_playing
                        || (snap.playing
                            && now.saturating_duration_since(root.checkpoint_at)
                                >= std::time::Duration::from_secs(5)));
                if checkpoint_due {
                    root.persist_checkpoint(&snap);
                }
                // 播放页展开时，歌词高亮跟着播放位置走；没展开就不用推进这份状态。
                if root.player_view_open {
                    root.sync_lyrics_position(cx);
                }
                if root.pending_track.is_none()
                    && !snap.error.is_empty()
                    && !snap.track_id.is_empty()
                    && root.playback_error.as_deref() != Some(snap.error.as_str())
                {
                    root.playback_error = Some(snap.error.clone());
                    let message = root.language.engine_error(&snap.error);
                    root.notify_error_once("playback-engine-failed", message);
                    cx.notify();
                }
                if let Some(error) = root.preferences_writer.take_error() {
                    let message = root.failure_text("保存播放设置失败", &error, SAVE_TEXT);
                    root.notify_error_once("preferences-save-failed", message);
                    cx.notify();
                }
                if let Some(error) = root.session_writer.take_error() {
                    let message = root.failure_text("保存播放会话失败", &error, SAVE_TEXT);
                    root.notify_error_once("session-save-failed", message);
                    cx.notify();
                }
                if snap.playing || root.pending_track.is_some() {
                    cx.notify();
                }
                // 播完自动下一首：按序号判断（循环同一首也不会漏切）。
                // 注意：正在装载下一首（pending）时「播完」属于旧曲目，
                // 只消费事件不推进——否则会把 pending 的那首跳过去（收尾瞬间
                // 手动切歌/插入队列时踩过）。
                // seek 之前积压的完成事件不能把当前曲目推进到下一首。
                let current = finished_event_is_current(
                    snap.finished_seq,
                    snap.seek_seq,
                    &mut root.last_finished_seq,
                    &mut root.last_seek_seq,
                );
                let should_advance = current;
                if snap.finished && should_advance && root.pending_track.is_none() {
                    root.advance_next(cx);
                }
            });
        })
        .detach();
    }

    /// 推进到下一首（用户点下一首或曲目自然播完）。
    fn advance_next(&mut self, cx: &mut Context<Self>) {
        // 切歌时丢掉进度预览，避免「拖到一半换歌」把 seek 用到新歌上。
        self.progress_preview = None;
        self.track_swap_dir = 1.0;
        self.queue.advance();
        self.start_track(cx);
    }

    /// 用户点了下一首（按钮 / 快捷键 / 媒体键）。
    pub fn next_track(&mut self, cx: &mut Context<Self>) {
        self.advance_next(cx);
    }

    /// 用户点了上一首。
    pub fn prev_track(&mut self, cx: &mut Context<Self>) {
        self.progress_preview = None;
        // 新内容从上方进入，和「下一首」的下方进入形成方向感。
        self.track_swap_dir = -1.0;
        self.queue.rewind();
        self.start_track(cx);
    }

    /// 开关双语歌词的翻译行。只改显示偏好，不动歌词数据与当前高亮。
    ///
    /// 译文行始终保留占位，这里只做 420ms 的透明度/位移过渡；英文的位置、
    /// 行高和滚动锚点都不变，所以播放中切换不会让歌词跳一下。
    pub(crate) fn toggle_lyrics_translation(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let duration = crate::ui::motion::duration::LYRIC_TRANSLATION_TOGGLE;
        let previous = self.settings.lyrics_translation;
        // 上一次动画还没走完就再次点击：从当前透明度接着走，不跳回端点。
        let current = if self.lyrics_translation_seq == 0 {
            f32::from(previous as u8)
        } else {
            let elapsed = self
                .lyrics_translation_at
                .map(|at| now.saturating_duration_since(at).as_secs_f32())
                .unwrap_or(duration.as_secs_f32());
            let t = (elapsed / duration.as_secs_f32()).clamp(0.0, 1.0);
            let target = f32::from(previous as u8);
            self.lyrics_translation_from + (target - self.lyrics_translation_from) * t
        };
        self.settings.lyrics_translation = !previous;
        self.lyrics_translation_from = current;
        self.lyrics_translation_seq = self.lyrics_translation_seq.wrapping_add(1);
        self.lyrics_translation_at = Some(now);
        // 和音量/播放模式走同一个后台单写者：点一下不阻塞在磁盘上。
        self.persist_playback_preferences();
        cx.notify();
    }

    /// 切到下一个播放模式：列表循环 → 单曲循环 → 随机播放（对齐官方轮换顺序）。
    pub fn cycle_play_mode(&mut self, cx: &mut Context<Self>) {
        self.queue.mode = self.queue.mode.next();
        // 换模式后重算随机顺序，避免沿用上一个模式留下的待播队列。
        self.queue.reset_shuffle_round();
        // 切到随机时立刻按当前歌单生成一整轮，否则面板要等下一次切歌才有内容。
        self.queue.prepare_shuffle_round();
        self.settings.play_mode = self.queue.mode;
        let label = self.tr(self.queue.mode.label());
        self.set_status("播放模式：{}", &[label.to_string()]);
        self.persist_playback_preferences();
        self.persist_session();
        cx.notify();
    }
}

/// seek 之前产生的完成事件已经失效；UI 只消费序号并跳过该事件。
fn finished_event_is_current(
    finished_seq: u64,
    seek_seq: u64,
    last_finished_seq: &mut u64,
    last_seek_seq: &mut u64,
) -> bool {
    let stale_finished = seek_seq != *last_seek_seq;
    *last_seek_seq = seek_seq;
    let current = finished_seq != *last_finished_seq && !stale_finished;
    // 只消费一次：成功处理过完成事件后回写，否则已上报的 finished 会在
    // 下一页心跳里继续返回 true，自动切换到下一首。
    if current {
        *last_finished_seq = finished_seq;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::finished_event_is_current;

    #[test]
    fn finished_event_is_consumed_exactly_once() {
        let mut last_finished = 0u64;
        let mut last_seek = 0u64;

        assert!(finished_event_is_current(
            1,
            0,
            &mut last_finished,
            &mut last_seek
        ));
        assert!(!finished_event_is_current(
            1,
            0,
            &mut last_finished,
            &mut last_seek
        ));

        // 同一首循环播完第二次：序号推进 → 再次触发。
        assert!(finished_event_is_current(
            2,
            0,
            &mut last_finished,
            &mut last_seek
        ));
    }

    #[test]
    fn finished_event_from_before_a_seek_is_skipped() {
        let mut last_finished = 0u64;
        let mut last_seek = 0u64;

        // UI 上一拍还停在 seek_seq=0；这一拍看到 (finished=1, seek=1)。
        // seek 序号变了 → 完成事件发生在 seek 之前，作废，不推进队列。
        assert!(!finished_event_is_current(
            1,
            1,
            &mut last_finished,
            &mut last_seek
        ));
        // 此后完成序号到达 last_finished：正常消费一次，然后失效。
        // （作废事件不回写 last_finished：防重复推进由 pending_track/play_seq 负责。）
        assert!(finished_event_is_current(
            1,
            1,
            &mut last_finished,
            &mut last_seek
        ));
        assert!(!finished_event_is_current(
            1,
            1,
            &mut last_finished,
            &mut last_seek
        ));
    }
}
