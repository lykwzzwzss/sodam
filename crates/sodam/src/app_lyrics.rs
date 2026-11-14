//! 歌词加载与播放页状态。
//!
//! 歌词在后台线程读取（内嵌标签 / 侧车 .lrc），回 UI 线程后整体替换。
//! 播放位置由心跳每 200ms 推进一次，这里只负责算当前行并把列表滚到居中。

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Context};

use crate::app::Root;
use sodam_core::lyrics::Lyrics;
use sodam_core::models::TrackItem;

impl Root {
    /// 为正在播放的曲目加载歌词。没有歌词时清空，播放页显示空态。
    pub(crate) fn request_lyrics(
        &mut self,
        track: &TrackItem,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let seq = self.lyrics_seq.wrapping_add(1);
        self.lyrics_seq = seq;
        self.lyrics = None;
        self.lyrics_layout = None;
        self.lyrics_for = Some(track.id.clone());
        self.lyrics_active = None;
        self.lyrics_prev_active = None;
        self.lyrics_switch_at = None;
        self.lyrics_scroll_line = None;
        self.lyrics_hold_until = None;
        let track_id = track.id.clone();
        let loaded = cx.background_spawn(async move { sodam_core::lyrics::load(&path) });
        cx.spawn(async move |this, cx| {
            let lyrics = loaded.await;
            let _ = this.update(cx, |root, cx| {
                if root.lyrics_seq != seq || root.lyrics_for.as_deref() != Some(track_id.as_str()) {
                    return;
                }
                root.lyrics = lyrics.map(Arc::new);
                root.lyrics_layout = None;
                root.lyrics_active = None;
                root.lyrics_prev_active = None;
                root.lyrics_switch_at = None;
                root.lyrics_scroll_line = None;
                root.sync_lyrics_position(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 按当前位置更新高亮行。滚动居中由渲染帧里的 `tick_lyrics_center` 做，
    /// 这里只改状态，避免在心跳线程里直接跳滚动位置。
    pub(crate) fn sync_lyrics_position(&mut self, cx: &mut Context<Self>) {
        let Some(lyrics) = self.lyrics.as_ref() else {
            self.lyrics_active = None;
            return;
        };
        let position_ms = (self.display_position_seconds().max(0.0) * 1000.0).round() as u64;
        let active = lyrics.active_index(position_ms);
        if active == self.lyrics_active {
            return;
        }
        self.lyrics_prev_active = self.lyrics_active;
        self.lyrics_active = active;
        self.lyrics_anim_seq = self.lyrics_anim_seq.wrapping_add(1);
        self.lyrics_switch_at = Some(std::time::Instant::now());
        // 换行后重新锚定；用户手动翻歌词的暂停时间仍然有效。
        self.lyrics_scroll_line = None;
        cx.notify();
    }

    /// 点击播放栏封面：展开大封面 + 歌词。
    pub(crate) fn open_player_view(&mut self, cx: &mut Context<Self>) {
        if self.player_view_open {
            return;
        }
        self.player_view_open = true;
        // 播放页是全屏内容，队列侧板让位。
        self.close_queue(cx);
        self.lyrics_active = None;
        self.lyrics_prev_active = None;
        self.lyrics_switch_at = None;
        self.lyrics_scroll_line = None;
        self.lyrics_hold_until = None;
        self.sync_lyrics_position(cx);
        cx.notify();
    }

    /// 返回普通页面。
    pub(crate) fn close_player_view(&mut self, cx: &mut Context<Self>) {
        if !self.player_view_open {
            return;
        }
        self.player_view_open = false;
        self.lyrics_scroll_line = None;
        cx.notify();
    }
}

/// 当前行开始染色后，用行时长的这个比例把整行染完；留一点余量，
/// 避免下一行开口时上一行还在慢慢爬。
const WIPE_PORTION: f32 = 0.92;

#[derive(Clone)]
pub(crate) struct LyricToken {
    pub(crate) text: String,
    pub(crate) weight: f32,
}

pub(crate) fn lyric_tokens(text: &str) -> Vec<LyricToken> {
    let mut tokens: Vec<String> = Vec::new();
    let mut word = String::new();
    for ch in text.chars() {
        if ch.is_whitespace() {
            // 空格归到它前面的那个词。注意必须先结束当前 word：否则
            // `What's the trick? I wish I knew` 里的空格会全部粘到 `?` 后面，
            // 表现为「句子中间空一大块、后面的单词挤在一起」。
            if !word.is_empty() {
                word.push(ch);
                tokens.push(std::mem::take(&mut word));
            } else if let Some(last) = tokens.last_mut() {
                last.push(ch);
            }
        } else if ch.is_ascii_alphanumeric() || ch == '\'' || ch == '’' || ch == '-' {
            word.push(ch);
        } else {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            tokens.push(ch.to_string());
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }

    tokens
        .into_iter()
        .map(|text| {
            let weight = text.chars().filter(|ch| !ch.is_whitespace()).count().max(1) as f32;
            LyricToken { text, weight }
        })
        .collect()
}

/// 当前行唱到哪了：0.0 开始、1.0 唱完。
pub(crate) fn line_progress(lyrics: &Lyrics, index: usize, position_ms: u64) -> f32 {
    let Some(start) = lyrics.lines.get(index).and_then(|line| line.at_ms) else {
        return 0.0;
    };
    let end = lyrics
        .lines
        .get(index + 1)
        .and_then(|line| line.at_ms)
        .unwrap_or(start.saturating_add(4_000));
    let span = end.saturating_sub(start).max(1) as f32;
    let elapsed = position_ms.saturating_sub(start) as f32;
    (elapsed / (span * WIPE_PORTION)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{line_progress, lyric_tokens};

    #[test]
    fn spaces_stay_with_the_preceding_word() {
        // 之前空格会全部粘到 `?` 后面：中间的空白越来越大，后面的单词挤在一起。
        let tokens = lyric_tokens("What's the trick? I wish I knew");
        let text: Vec<&str> = tokens.iter().map(|token| token.text.as_str()).collect();
        assert_eq!(
            text,
            vec!["What's ", "the ", "trick", "? ", "I ", "wish ", "I ", "knew"]
        );
    }

    #[test]
    fn tokens_split_chinese_per_character_and_latin_per_word() {
        let tokens = lyric_tokens("一点归鸿 (DJ版)");
        let text: Vec<&str> = tokens.iter().map(|token| token.text.as_str()).collect();
        assert_eq!(text, vec!["一", "点", "归", "鸿 ", "(", "DJ", "版", ")"]);
    }

    #[test]
    fn progress_clamps_to_the_line_span() {
        let lyrics =
            sodam_core::lyrics::parse("[00:01.00]第一句\n[00:03.00]第二句\n").expect("歌词");
        assert_eq!(line_progress(&lyrics, 0, 500), 0.0);
        assert!(line_progress(&lyrics, 0, 2_000) > 0.4);
        assert_eq!(line_progress(&lyrics, 0, 9_000), 1.0);
    }
}
