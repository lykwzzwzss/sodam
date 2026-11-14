//! 播放队列（列表循环 / 单曲循环 / 随机播放）。

use crate::models::TrackItem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlayMode {
    /// 列表循环：播到末尾回到第一首。
    #[default]
    #[serde(alias = "Sequential")]
    ListLoop,
    /// 单曲循环：停在当前曲目。
    RepeatOne,
    /// 随机播放：先进队后随机取下一首（见 `Queue::history`）。
    Shuffle,
}

impl PlayMode {
    pub fn label(&self) -> &'static str {
        match self {
            PlayMode::ListLoop => "列表循环",
            PlayMode::RepeatOne => "单曲循环",
            PlayMode::Shuffle => "随机播放",
        }
    }

    /// 按官方的轮换顺序切到下一个模式：列表循环 → 单曲循环 → 随机播放。
    pub fn next(self) -> Self {
        match self {
            PlayMode::ListLoop => PlayMode::RepeatOne,
            PlayMode::RepeatOne => PlayMode::Shuffle,
            PlayMode::Shuffle => PlayMode::ListLoop,
        }
    }

    /// 图标名（`ui/icons.rs` 里的 svg 名）。
    pub fn icon(&self) -> &'static str {
        match self {
            PlayMode::ListLoop => "repeat",
            PlayMode::RepeatOne => "repeat-1",
            PlayMode::Shuffle => "shuffle",
        }
    }
}

/// 队列的可保存状态：当前项、随机历史与待播轮次。
///
/// 不含曲目列表也不含播放模式：曲目由本地曲库按 id 还原，模式存在配置里。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueState {
    pub index: usize,
    pub history: Vec<usize>,
    pub shuffle_round: Vec<usize>,
    pub redo: Option<usize>,
}

/// 播放队列：维护曲目列表与当前下标。
#[derive(Debug, Clone, Default)]
pub struct Queue {
    /// 队列版本号：每次变更 +1，UI 用它判断需不需要重建缓存快照。
    revision: u64,
    tracks: Vec<TrackItem>,
    index: usize,
    pub mode: PlayMode,
    /// 随机播放的历史栈：记录走过的下标，`prev`/`rewind` 沿它回退。
    ///
    /// 官方随机播放是「先入队再随机」，而不是把队列洗牌——队列抽屉里
    /// 仍显示真实曲目顺序，只有播放分支是随机的。没有这个栈时
    /// 「上一首」只能退到列表里的前一首，与随机播放的语义不符。
    history: Vec<usize>,
    /// 随机播放的待播顺序（下标）。一轮内不重复，播完再生成新一轮。
    ///
    /// 预先生成而不是每次临时抽取，是为了让队列面板能显示「接下来会播什么」：
    /// 临时抽取时下一首只有播放那一刻才知道，面板只能显示原歌单顺序。
    shuffle_round: Vec<usize>,
    /// 「上一首」退回去之后，下一次「下一首」优先回到被放弃的那首。
    ///
    /// 没有它时「上一首 → 下一首」会在随机模式下跳到别处，原本该播的那首
    /// 也从本轮里消失了。
    redo: Option<usize>,
}

/// 打散时间种子：splitmix64 的收尾混合。
///
/// 直接用纳秒时间戳取模会让低位几乎没有变化——连续调用间隔只有微秒级，
/// `seed % len` 会反复落在同一个值上。混一轮之后低位分布才够散。
fn mix_seed(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

/// 把一组下标随机打散（Fisher-Yates，种子先混一次）。
///
/// 纳秒低位在连续调用间变化太小，直接取模会退化，所以每步都再混一轮。
fn shuffled_indices(mut round: Vec<usize>) -> Vec<usize> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    let mut state = mix_seed(seed);
    for i in (1..round.len()).rev() {
        state = mix_seed(state);
        let pick = (state % (i as u64 + 1)) as usize;
        round.swap(i, pick);
    }
    round
}

impl Queue {
    pub fn new(tracks: Vec<TrackItem>) -> Self {
        Self {
            revision: 0,
            tracks,
            index: 0,
            mode: PlayMode::default(),
            history: Vec::new(),
            shuffle_round: Vec::new(),
            redo: None,
        }
    }

    pub fn tracks(&self) -> &[TrackItem] {
        &self.tracks
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn current(&self) -> Option<&TrackItem> {
        self.tracks.get(self.index)
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// 队列版本号（UI 缓存用）：只在曲目列表本身变化时 +1。
    ///
    /// 切歌只改当前下标，列表不变，所以 `advance`/`rewind`/`jump` 不 bump——
    /// 否则每次切歌都要把整条队列深拷贝一次（上万首时白卡一帧）。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 导出可保存状态（随机历史、待播轮次一起带上）。
    pub fn state(&self) -> QueueState {
        QueueState {
            index: self.index,
            history: self.history.clone(),
            shuffle_round: self.shuffle_round.clone(),
            redo: self.redo,
        }
    }

    /// 用保存的状态替换整条队列（启动恢复用）。
    ///
    /// 下标按当前曲目数过滤：会话保存之后曲库变小、文件被删时，
    /// 旧下标会指向不存在的曲目，留着会让「下一首」跳过或越界。
    pub fn restore(&mut self, tracks: Vec<TrackItem>, state: QueueState) {
        self.bump();
        self.tracks = tracks;
        let len = self.tracks.len();
        self.index = state.index.min(len.saturating_sub(1));
        self.history = state.history.into_iter().filter(|i| *i < len).collect();
        self.shuffle_round = state
            .shuffle_round
            .into_iter()
            .filter(|i| *i < len)
            .collect();
        self.redo = state.redo.filter(|i| *i < len);
    }

    /// 换一批曲目。随机历史与回退意图一起清空：新队列是一轮新的播放。
    ///
    /// `mode` 不清空——模式是用户的选择，换歌单不该把它重置掉（#12）。
    pub fn replace(&mut self, tracks: Vec<TrackItem>, start: usize) {
        self.bump();
        self.tracks = tracks;
        self.index = start.min(self.tracks.len().saturating_sub(1));
        self.history.clear();
        self.shuffle_round.clear();
        self.redo = None;
    }

    /// 下一首。
    ///
    /// * 列表循环：下标 +1，到末尾回到第一首；
    /// * 单曲循环：停在原地（由播放器决定是否重播）；
    /// * 随机播放：把当前下标压入历史，再从「未播放过」的曲目里随机挑一首。
    pub fn advance(&mut self) -> Option<&TrackItem> {
        if self.tracks.is_empty() {
            return None;
        }
        match self.mode {
            PlayMode::RepeatOne => {}
            PlayMode::ListLoop => {
                self.index = (self.index + 1) % self.tracks.len();
            }
            PlayMode::Shuffle => {
                self.history.push(self.index);
                // 「上一首」之后按「下一首」应回到被放弃的那首，而不是重新随机。
                if let Some(redo) = self.redo.take() {
                    // `rewind` 把它插回了轮次首位；回到它之后要摘掉，
                    // 否则「接下来」会重复显示，下一轮还会再播一次。
                    self.shuffle_round.retain(|index| *index != redo);
                    self.index = redo;
                    if self.shuffle_round.is_empty() {
                        // 轮次只剩被放弃的那首：回到它之后立刻补满新一轮，
                        // 面板的「接下来」不能出现空档（与下面正常推进一致）。
                        self.history.clear();
                        self.shuffle_round = self.build_shuffle_round();
                    }
                    return self.current();
                }
                if let Some(next) = self.next_shuffle_index() {
                    self.index = next;
                    // 本轮播完就立刻开新一轮：面板的「接下来」不该出现空档，
                    // 否则用户会以为没歌了。清历史保证「上一首」不跨轮回退。
                    if self.shuffle_round.is_empty() {
                        self.history.clear();
                        self.shuffle_round = self.build_shuffle_round();
                    }
                }
            }
        }
        self.current()
    }

    /// 取随机播放的下一个下标，并推进 `shuffle_round`。
    ///
    /// `shuffle_round` 若为空就现生成一轮：除当前曲目外的全部下标随机排列。
    /// 一轮播完后重新生成，因此不会出现「剩下最后一首反复播」。
    fn next_shuffle_index(&mut self) -> Option<usize> {
        let len = self.tracks.len();
        if len <= 1 {
            return None;
        }
        if self.shuffle_round.is_empty() {
            self.shuffle_round = self.build_shuffle_round();
        }
        if self.shuffle_round.is_empty() {
            return None;
        }
        let picked = self.shuffle_round.remove(0);
        Some(picked)
    }

    /// 生成一轮随机顺序（不含当前曲目）。
    fn build_shuffle_round(&self) -> Vec<usize> {
        let round: Vec<usize> = (0..self.tracks.len())
            .filter(|index| *index != self.index)
            .collect();
        shuffled_indices(round)
    }

    /// 清空随机待播顺序，下次推进时重新生成一轮。
    ///
    /// 切换播放模式后必须调用：否则会沿用上一个模式留下的待播队列，
    /// 面板显示的「接下来」与实际播放不符。
    pub fn reset_shuffle_round(&mut self) {
        self.shuffle_round.clear();
        self.history.clear();
        self.redo = None;
    }

    /// 立刻生成一轮随机待播顺序（随机模式下调用）。
    ///
    /// 切到随机、或整张歌单点「随机播放」入队后调用，让队列面板
    /// 马上显示真实的「接下来」，而不是等下一次切歌才生成。
    pub fn prepare_shuffle_round(&mut self) {
        if self.mode == PlayMode::Shuffle && self.tracks.len() > 1 {
            self.shuffle_round = self.build_shuffle_round();
        }
    }

    /// 「接下来」的真实播放顺序（供队列面板显示用）。
    ///
    /// * 列表循环：当前项之后到末尾，再绕回开头；
    /// * 随机播放：`shuffle_round` 里尚未播的部分；
    /// * 单曲循环：空（一直播当前这首）。
    ///
    /// 返回的是下标，调用方用 `tracks()[i]` 取曲目。
    pub fn upcoming(&self) -> Vec<usize> {
        if self.tracks.is_empty() {
            return Vec::new();
        }
        match self.mode {
            PlayMode::RepeatOne => Vec::new(),
            PlayMode::ListLoop => {
                // 不回绕到当前项之前：那部分由抽屉的「已播放」段渲染，
                // 环绕会让同一首歌出现两行。列表循环真正的回绕发生在
                // 播到末尾之后，届时「已播放」会重新变空。
                let len = self.tracks.len();
                (self.index + 1..len).collect()
            }
            PlayMode::Shuffle => self
                .shuffle_round
                .iter()
                .copied()
                .filter(|index| *index < self.tracks.len())
                .collect(),
        }
    }

    /// 从队列里移除某一项（右键菜单用）。
    ///
    /// **不允许移除当前项**：正在播的这首还挂在引擎上，移除它会让
    /// 「队列当前项」与「引擎正在播的」分叉，自动切歌的推进规则全得跟着
    /// 特判（实测踩过：后继曲目会被跳过）。想换歌直接点下一首。
    ///
    /// 移除当前项之前的项则下标左移，保证仍指向同一首歌。
    pub fn remove(&mut self, index: usize) -> Option<TrackItem> {
        if index >= self.tracks.len() || index == self.index {
            return None;
        }
        let removed = self.tracks.remove(index);
        if index < self.index {
            self.index -= 1;
        }
        // 随机轮次和历史栈存的是下标，移除会让后面的项整体左移一位：
        // 不同步的话，轮次会指向错的曲目，末尾还会留下越界下标（被静默跳过，
        // 表现为「少播一首」）。
        self.shift_indices_for_removal(index);
        self.bump();
        Some(removed)
    }

    /// 移除第 `removed` 项后，同步 `shuffle_round` 与 `history` 里的下标。
    fn shift_indices_for_removal(&mut self, removed: usize) {
        let adjust = |value: &mut usize| {
            if *value > removed {
                *value -= 1;
            }
        };
        self.shuffle_round.retain(|value| *value != removed);
        self.shuffle_round.iter_mut().for_each(adjust);
        self.history.retain(|value| *value != removed);
        self.history.iter_mut().for_each(adjust);
        match self.redo {
            Some(value) if value == removed => self.redo = None,
            Some(value) if value > removed => self.redo = Some(value - 1),
            _ => {}
        }
    }

    /// 上一首。
    ///
    /// * 列表循环：下标 -1，到开头回到队尾；
    /// * 随机播放：沿历史栈回退到真正播过的那首；
    /// * 单曲循环：同样沿历史栈回退（用户点「上一首」是想换歌）。
    pub fn rewind(&mut self) -> Option<&TrackItem> {
        if self.tracks.is_empty() {
            return None;
        }
        if self.mode == PlayMode::Shuffle {
            if let Some(previous) = self.history.pop() {
                if previous < self.tracks.len() {
                    // 记下被放弃的那首，下一次「下一首」原路返回它。
                    self.redo = Some(self.index);
                    // 它重新回到待播轮次的最前面，面板与「下一首」保持一致。
                    if !self.shuffle_round.contains(&self.index) && self.index != previous {
                        self.shuffle_round.insert(0, self.index);
                    }
                    self.index = previous;
                    return self.current();
                }
            }
        }
        self.index = if self.index == 0 {
            self.tracks.len() - 1
        } else {
            self.index - 1
        };
        self.current()
    }

    /// 跳到指定下标（用户点队列里的某一首）。
    ///
    /// 随机播放时这次跳转也会进历史栈，之后「上一首」能退回原来那首。
    pub fn jump(&mut self, index: usize) -> Option<&TrackItem> {
        if index < self.tracks.len() {
            if self.mode == PlayMode::Shuffle && index != self.index {
                self.history.push(self.index);
                // 用户手动跳转后，「下一首」应继续当前轮次，而不是回到跳转前放弃的那首。
                self.redo = None;
                // 用户点了「接下来」里的某一首：它已经播了，必须从待播轮次里去掉，
                // 否则 `advance` 会把它再播一次。
                self.shuffle_round.retain(|value| *value != index);
            }
            self.index = index;
        }
        self.current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(n: usize) -> Vec<TrackItem> {
        (0..n)
            .map(|i| TrackItem {
                id: i.to_string(),
                title: format!("t{i}"),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn queue_navigates_and_wraps() {
        let mut queue = Queue::new(tracks(3));
        assert_eq!(queue.current().unwrap().title, "t0");
        assert_eq!(queue.advance().unwrap().title, "t1");
        assert_eq!(queue.rewind().unwrap().title, "t0");
        assert_eq!(queue.rewind().unwrap().title, "t2");
        assert_eq!(queue.jump(1).unwrap().title, "t1");
    }

    #[test]
    fn repeat_one_stays_on_track() {
        let mut queue = Queue::new(tracks(2));
        queue.mode = PlayMode::RepeatOne;
        assert_eq!(queue.advance().unwrap().title, "t0");
        assert_eq!(PlayMode::ListLoop.next(), PlayMode::RepeatOne);
        assert_eq!(PlayMode::RepeatOne.next(), PlayMode::Shuffle);
        assert_eq!(PlayMode::Shuffle.next(), PlayMode::ListLoop);
    }

    #[test]
    fn shuffle_never_repeats_before_covering_the_queue() {
        let mut queue = Queue::new(tracks(6));
        queue.mode = PlayMode::Shuffle;
        let mut visited = vec![queue.index()];
        for _ in 0..5 {
            queue.advance();
            visited.push(queue.index());
        }
        // 一轮里每首都该恰好出现一次。
        visited.sort_unstable();
        assert_eq!(visited, vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn shuffle_rewind_returns_to_previously_played_track() {
        let mut queue = Queue::new(tracks(6));
        queue.mode = PlayMode::Shuffle;
        let first = queue.index();
        queue.advance();
        let second = queue.index();
        assert_ne!(first, second);
        // 「上一首」沿历史栈退回真正播过的那首。
        assert_eq!(queue.rewind().unwrap().id, first.to_string());
        // 再前进时重新随机，并且不会挑到本轮已经播过的 `first`（历史里还留着它）。
        let third = queue.index();
        queue.advance();
        assert_ne!(queue.index(), first, "本轮不重复已播曲目");
        assert_ne!(queue.index(), third);
    }

    #[test]
    fn shuffle_starts_new_round_after_full_cycle() {
        let mut queue = Queue::new(tracks(3));
        queue.mode = PlayMode::Shuffle;
        // 播完一整轮后应自动开新一轮，不会卡在最后一首、也不会把「接下来」留空。
        for _ in 0..3 {
            queue.advance();
        }
        assert!(queue.current().is_some());
        assert!(
            !queue.upcoming().is_empty(),
            "一轮结束后应立即生成下一轮，面板不能是空的"
        );
        let before = queue.index();
        queue.advance();
        assert_eq!(
            queue.history.len(),
            0,
            "新一轮开始时应清空上一轮的历史，避免跨轮回退"
        );
        assert!(before != queue.index() || queue.len() == 1);
    }

    #[test]
    fn list_loop_wraps_at_the_end() {
        let mut queue = Queue::new(tracks(3));
        queue.mode = PlayMode::ListLoop;
        assert_eq!(queue.advance().unwrap().id, "1");
        assert_eq!(queue.advance().unwrap().id, "2");
        assert_eq!(queue.advance().unwrap().id, "0");
    }

    #[test]
    fn upcoming_matches_actual_play_order_in_shuffle() {
        let mut queue = Queue::new(tracks(6));
        queue.mode = PlayMode::Shuffle;
        // 先推进一次，生成随机轮次。
        queue.advance();
        let shown: Vec<usize> = queue.upcoming();
        let mut actual = Vec::new();
        for _ in 0..shown.len() {
            queue.advance();
            actual.push(queue.index());
        }
        assert_eq!(shown, actual, "面板显示的「接下来」必须等于真实播放顺序");
    }

    #[test]
    fn upcoming_never_shows_empty_mid_playback() {
        let mut queue = Queue::new(tracks(4));
        queue.mode = PlayMode::Shuffle;
        queue.prepare_shuffle_round();
        // 连续推进多轮：任何时候「接下来」都不该是空的（面板会显示成没有歌）。
        for step in 0..20 {
            assert!(
                !queue.upcoming().is_empty(),
                "第 {step} 次推进后面板的「接下来」空了"
            );
            queue.advance();
        }
    }

    #[test]
    fn jumping_to_upcoming_track_removes_it_from_round() {
        let mut queue = Queue::new(tracks(6));
        queue.mode = PlayMode::Shuffle;
        queue.prepare_shuffle_round();
        let target = queue.upcoming()[0];
        queue.jump(target);
        assert!(
            !queue.upcoming().contains(&target),
            "点了「接下来」里的歌之后，它不应再留在待播轮次里（否则会重播）"
        );
    }

    #[test]
    fn rewind_then_advance_returns_to_abandoned_track() {
        let mut queue = Queue::new(tracks(5));
        queue.mode = PlayMode::Shuffle;
        queue.prepare_shuffle_round();
        let first = queue.index();
        queue.advance();
        let second = queue.index();
        assert_ne!(first, second);
        queue.rewind();
        assert_eq!(queue.index(), first, "应退回上一首");
        queue.advance();
        assert_eq!(queue.index(), second, "再按下一首应回到刚放弃的那首");
        assert!(
            !queue.upcoming().contains(&second),
            "回到被放弃的那首后，它不应再留在待播轮次里"
        );
        assert!(
            !queue.upcoming().is_empty(),
            "回到被放弃的那首后要立刻补满新一轮，面板不能空"
        );
    }

    #[test]
    fn upcoming_is_empty_for_repeat_one() {
        let mut queue = Queue::new(tracks(3));
        queue.mode = PlayMode::RepeatOne;
        assert!(queue.upcoming().is_empty());
        queue.advance();
        assert_eq!(queue.index(), 0);
    }

    #[test]
    fn upcoming_lists_rest_of_queue_for_list_loop() {
        let mut queue = Queue::new(tracks(3));
        queue.mode = PlayMode::ListLoop;
        queue.jump(1);
        // 不回绕到当前项之前：那部分由抽屉的「已播放」段渲染，重复会出两行。
        assert_eq!(queue.upcoming(), vec![2]);
    }

    #[test]
    fn switching_mode_resets_shuffle_round() {
        let mut queue = Queue::new(tracks(6));
        queue.mode = PlayMode::Shuffle;
        queue.advance();
        assert!(!queue.upcoming().is_empty());
        queue.reset_shuffle_round();
        assert!(queue.upcoming().is_empty(), "重置后要等下一次推进再生成");
    }

    #[test]
    fn switching_from_repeat_one_to_shuffle_covers_whole_queue() {
        let mut queue = Queue::new(tracks(10));
        // 模拟用户在单曲循环里听了一段时间（当前项仍是同一首）。
        queue.mode = PlayMode::RepeatOne;
        for _ in 0..5 {
            queue.advance();
        }
        assert_eq!(queue.index(), 0);

        // 点两下切到随机（单曲 → 随机），应当立刻按整个歌单生成轮次。
        queue.mode = queue.mode.next();
        queue.reset_shuffle_round();
        queue.prepare_shuffle_round();

        let mut upcoming = queue.upcoming();
        assert_eq!(
            upcoming.len(),
            9,
            "应覆盖除当前曲目外的全部 9 首，实际 {:?}",
            upcoming
        );
        upcoming.sort_unstable();
        assert_eq!(upcoming, (1..10).collect::<Vec<_>>());
    }

    #[test]
    fn prepare_shuffle_round_fills_upcoming_immediately() {
        let mut queue = Queue::new(tracks(8));
        queue.mode = PlayMode::Shuffle;
        queue.jump(3);
        assert!(queue.upcoming().is_empty(), "尚未生成轮次");
        queue.prepare_shuffle_round();
        let upcoming = queue.upcoming();
        assert_eq!(upcoming.len(), 7, "除当前曲目外的全部曲目");
        assert!(!upcoming.contains(&3), "轮次里不含当前曲目");
    }

    #[test]
    fn remove_keeps_shuffle_round_valid() {
        let mut queue = Queue::new(tracks(8));
        queue.mode = PlayMode::Shuffle;
        queue.prepare_shuffle_round();
        let removed = queue.remove(2).expect("可以移除非当前项");
        assert_eq!(removed.id, "2");

        let upcoming = queue.upcoming();
        // 下标必须都在合法范围内，且不包含被移除后仍指向旧位置的值。
        assert!(
            upcoming.iter().all(|index| *index < queue.len()),
            "轮次里出现越界下标：{:?}（队列长度 {}）",
            upcoming,
            queue.len()
        );
        assert_eq!(upcoming.len(), queue.len() - 1, "剩余曲目应全部在轮次里");
        let mut sorted = upcoming.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), upcoming.len(), "轮次里不该有重复下标");
    }

    #[test]
    fn removing_before_redo_keeps_it_pointing_at_the_same_track() {
        let mut queue = Queue::new(tracks(4));
        queue.mode = PlayMode::Shuffle;
        queue.jump(2);
        queue.rewind();
        assert_eq!(queue.redo, Some(2));
        // 移除 redo 之前的曲目：后面的下标整体左移，redo 也要跟着 -1。
        queue.remove(1).expect("remove");
        assert_eq!(queue.redo, Some(1));
        assert_eq!(queue.advance().unwrap().id, "2");
    }

    #[test]
    fn replacing_the_queue_or_jumping_clears_stale_redo() {
        let mut queue = Queue::new(tracks(4));
        queue.mode = PlayMode::Shuffle;
        queue.jump(3);
        queue.rewind();
        assert!(queue.redo.is_some());
        // 新队列只有 2 首：旧的 redo 下标不能再被当成有效目标。
        queue.replace(tracks(2), 0);
        assert_eq!(queue.redo, None);
        assert!(queue.advance().is_some());

        let mut queue = Queue::new(tracks(4));
        queue.mode = PlayMode::Shuffle;
        queue.jump(3);
        queue.rewind();
        queue.jump(1);
        assert_eq!(queue.redo, None, "手动跳转后不应再回到跳转前放弃的曲目");
    }

    #[test]
    fn revision_only_changes_when_the_track_list_changes() {
        let mut queue = Queue::new(tracks(4));
        let start = queue.revision();
        queue.advance();
        queue.rewind();
        queue.jump(2);
        assert_eq!(
            queue.revision(),
            start,
            "切歌只改下标，不该让 UI 重建整条队列"
        );
        queue.remove(0);
        assert_ne!(queue.revision(), start, "移除曲目必须让缓存失效");
        let before_replace = queue.revision();
        queue.replace(tracks(2), 0);
        assert_ne!(queue.revision(), before_replace);
    }

    #[test]
    fn empty_queue_is_safe() {
        let mut queue = Queue::default();
        assert!(queue.is_empty());
        assert!(queue.advance().is_none());
        assert!(queue.rewind().is_none());
        assert!(queue.current().is_none());
    }

    #[test]
    fn state_round_trips_shuffle_history() {
        let mut queue = Queue::new(tracks(6));
        queue.mode = PlayMode::Shuffle;
        queue.prepare_shuffle_round();
        queue.advance();
        queue.advance();
        let state = queue.state();
        let index = queue.index();

        let mut restored = Queue::new(tracks(6));
        restored.mode = PlayMode::Shuffle;
        restored.restore(queue.tracks().to_vec(), state.clone());
        assert_eq!(restored.index(), index);
        assert_eq!(restored.state(), state);
        assert_eq!(restored.upcoming(), queue.upcoming(), "待播顺序必须保持");
        // 上一首沿历史栈退回，恢复后仍然成立。
        let previous = queue.state().history.last().copied();
        restored.rewind();
        assert_eq!(Some(restored.index()), previous);
    }

    #[test]
    fn restore_filters_out_of_range_indices() {
        let mut queue = Queue::new(tracks(2));
        let state = QueueState {
            index: 9,
            history: vec![0, 5],
            shuffle_round: vec![0, 1, 7],
            redo: Some(4),
        };
        queue.restore(tracks(2), state);
        assert_eq!(queue.index(), 1);
        assert_eq!(queue.state().history, vec![0]);
        assert_eq!(queue.state().shuffle_round, vec![0, 1]);
        assert_eq!(queue.state().redo, None);
        assert!(queue.advance().is_some());
    }

    #[test]
    fn remove_rejects_current_but_shifts_earlier_indices() {
        let mut queue = Queue::new(tracks(3));
        queue.jump(1); // 当前 t1
                       // 当前项不允许移除：它还挂在引擎上，移除会造成状态分叉
        assert!(queue.remove(1).is_none());
        assert_eq!(queue.len(), 3);
        // 移除当前项之前的项：下标左移，仍指向 t1
        assert_eq!(queue.remove(0).unwrap().title, "t0");
        assert_eq!(queue.current().unwrap().title, "t1");
        assert_eq!(queue.index(), 0);
        // 移除之后的项不影响当前项
        assert_eq!(queue.remove(1).unwrap().title, "t2");
        assert_eq!(queue.current().unwrap().title, "t1");
        // 越界拒绝
        assert!(queue.remove(9).is_none());
    }
}
