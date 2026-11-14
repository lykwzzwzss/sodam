//! 底部状态行：`Root::status_line` 的唯一渲染出口（#83）。
//!
//! 普通操作反馈（切档、开关、收藏、加载中…）有六十多处写入点。它们以前只写不画，
//! 用户看不到；这里给它们一个固定位置：播放栏上方左侧一行，停留 [`VISIBLE_FOR`]
//! 后自行消失。重要的、需要打断的事件仍然走 [`crate::ui::message`] 的全局轻提示，
//! 两条通道各管一件事——轻提示负责「叫住你」，状态行负责「留下来说过什么」。
//!
//! 动效：出现 160ms 淡入 + 上移 4px，替换时旧文案 140ms 淡出，
//! 到期后先播 140ms 退出动画再卸载（不再瞬间消失）。

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, AnimationExt, AnyElement, Context, ElementId};

use crate::app::Root;
use crate::ui::motion;
use crate::ui::theme;

/// 状态行停留时长。
///
/// 普通操作反馈 3 秒足够读完；比旧版的 5 秒短，避免一条旧文案一直挂在
/// 播放栏上方。真正需要用户处理的错误另有 3 秒轻提示 + 常驻错误条。
pub const VISIBLE_FOR: Duration = Duration::from_secs(3);

/// 状态行的全部状态：文案、写入序号、写入时刻、已排过计时器的序号。
///
/// 四者必须一起变：单独写文案而不动序号，到期计时器就还在替旧文案计时。
/// 收在一个类型里，写入方只调 [`StatusLine::set`]，看不到这些细节。
#[derive(Default)]
pub(crate) struct StatusLine {
    text: String,
    /// 替换前的文案：只在交叉淡化窗口内渲染。
    previous: Option<String>,
    /// 每写一条状态加一；计时器拿它判断「到期时还是不是同一条」。
    seq: u64,
    /// 写入时刻。`None` = 没有要显示的内容。
    since: Option<Instant>,
    /// 已经为哪个 `seq` 排过计时器。渲染每帧都跑，靠它避免每帧起一个后台任务。
    scheduled_seq: u64,
    /// 已过显示窗口、正在播放退出动画。
    exiting: bool,
}

impl StatusLine {
    /// 写一条新状态：顶掉旧文案，重新开始计时。空文案等同于清空。
    pub(crate) fn set(&mut self, text: String) {
        let old = std::mem::take(&mut self.text);
        self.previous =
            (self.since.is_some() && !old.trim().is_empty() && old.trim() != text.trim())
                .then_some(old);
        self.seq = self.seq.wrapping_add(1);
        self.since = Some(Instant::now());
        self.text = text;
        self.exiting = false;
    }

    /// 清空。切页时调用：状态行是全局反馈，不属于某一页，
    /// 留在新页面上就成了上一页的残留文案。
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.previous = None;
        self.since = None;
        self.exiting = false;
    }

    /// 现在该显示什么：空文案、没写过、或已过显示窗口（含退出动画）都不显示。
    pub(crate) fn visible(&mut self, now: Instant) -> Option<&str> {
        let since = self.since?;
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= VISIBLE_FOR {
            self.exiting = true;
        }
        if elapsed >= VISIBLE_FOR + motion::duration::STATUS_OUT {
            return None;
        }
        let text = self.text.trim();
        (!text.is_empty()).then_some(text)
    }

    /// 为当前这条状态认领一次到期计时：同一条状态只认领一次。
    ///
    /// 返回 `(序号, 剩余时长)`。剩余按**写入时刻**算，不按首次渲染时刻重新起算：
    /// 没有别的重绘时，收走这一行靠的就是这个计时器到期后的 `notify`；首帧来得越晚，
    /// 按整段重排就会让它多挂越久。
    pub(crate) fn claim_timer(&mut self, now: Instant) -> Option<(u64, Duration)> {
        if self.scheduled_seq == self.seq {
            return None;
        }
        let since = self.since?;
        self.scheduled_seq = self.seq;
        Some((
            self.seq,
            VISIBLE_FOR.saturating_sub(now.saturating_duration_since(since)),
        ))
    }

    /// 计时到期：如果到期时还是同一条状态，进入退出动画并返回 `true`。
    ///
    /// 文案先保留：渲染侧还要用它播 140ms 的淡出。
    pub(crate) fn expire(&mut self, seq: u64) -> bool {
        if self.seq != seq || self.since.is_none() || self.exiting {
            return false;
        }
        self.exiting = true;
        true
    }

    /// 退出动画结束：真正收走文案。
    pub(crate) fn finish_exit(&mut self, seq: u64) -> bool {
        if self.seq != seq || !self.exiting {
            return false;
        }
        self.clear();
        true
    }
}

impl Root {
    /// 给当前这条状态挂到期计时器；计时器在渲染路径上按序号去重。
    ///
    /// 到期后先标记退出、重绘一次播淡出，再等一个动画时长真正清空。
    pub(crate) fn schedule_status_clear(&mut self, cx: &mut Context<Self>) {
        let Some((seq, remaining)) = self.status_line.claim_timer(Instant::now()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(remaining).await;
            let exiting = this.update(cx, |root, cx| {
                if root.status_line.expire(seq) {
                    cx.notify();
                }
                root.status_line.exiting
            });
            if !exiting.unwrap_or(false) {
                return;
            }
            cx.background_executor()
                .timer(motion::duration::STATUS_OUT)
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.status_line.finish_exit(seq) {
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// 状态行卡片的统一样式；新旧两张卡片必须完全一致，交叉淡化才不跳。
fn card(text: String) -> AnyElement {
    div()
        .max_w(px(theme::STATUS_LINE_MAX_W))
        .truncate()
        .px(px(theme::space::MD))
        .py(px(theme::space::SM))
        .rounded(px(theme::radius::CARD))
        .border_1()
        .border_color(theme::border())
        // 状态行会压在列表上：用磨砂深底而不是 7% 白，下面的歌名不会透上来。
        .bg(theme::glass_panel())
        .backdrop_blur(px(18.0))
        .shadow_lg()
        .text_size(theme::Text::Small.size())
        .text_color(theme::text())
        .child(text)
        .into_any_element()
}

/// 渲染状态行；没有可显示的内容时返回 `None`。
///
/// 位置固定在播放栏上方、内容区左侧：与播放栏同一个视觉底部，又不挤占播放栏
/// 自己的三栏布局。用绝对定位，出现与消失都不推挤页面。
///
/// 它不挂任何交互处理器（与 `ui::message` 的浮层同型），预期不会产生命中框，
/// 压在页面上的这几秒不该挡住下面列表的点击——这一点要在真机验收时确认。
pub(crate) fn render(root: &mut Root, cx: &mut Context<Root>) -> Option<AnyElement> {
    let text = root.status_line.visible(Instant::now())?.to_string();
    root.schedule_status_clear(cx);
    let seq = root.status_line.seq;
    let exiting = root.status_line.exiting;
    let previous = root
        .status_line
        .previous
        .clone()
        .filter(|previous| previous.trim() != text.trim());

    let mut stack = div()
        .id("status-line")
        .absolute()
        .bottom(px(theme::PLAYER_H + theme::space::SM))
        .left(px(theme::SIDEBAR_W + theme::space::LG));
    if let Some(previous) = previous {
        // 旧文案绝对定位在下层，140ms 淡出；新文案决定容器尺寸。
        stack = stack.child(
            div()
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0))
                .w_full()
                .with_animation(
                    ElementId::Name(format!("status-out-old-{seq}").into()),
                    motion::standard(motion::duration::STATUS_OUT),
                    |this, delta| this.opacity(1.0 - delta),
                )
                .child(card(previous)),
        );
    }

    let new_card = div().child(card(text));
    let new_card = if exiting {
        new_card
            .with_animation(
                ElementId::Name(format!("status-exit-{seq}").into()),
                motion::standard(motion::duration::STATUS_OUT),
                |this, delta| this.opacity(1.0 - delta).top(px(-4.0 * delta)),
            )
            .into_any_element()
    } else {
        new_card
            .with_animation(
                ElementId::Name(format!("status-enter-{seq}").into()),
                motion::standard(motion::duration::STATUS_IN),
                |this, delta| this.opacity(delta).top(px(4.0 * (1.0 - delta))),
            )
            .into_any_element()
    };
    Some(stack.child(new_card).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_status_is_visible() {
        let mut line = StatusLine::default();
        line.set("已收藏：晴天".to_string());
        assert_eq!(line.visible(Instant::now()), Some("已收藏：晴天"));
    }

    #[test]
    fn empty_or_never_written_status_never_shows() {
        let mut line = StatusLine::default();
        assert_eq!(line.visible(Instant::now()), None);
        line.set("   ".to_string());
        assert_eq!(line.visible(Instant::now()), None);
    }

    #[test]
    fn expired_status_plays_exit_before_disappearing() {
        let mut line = StatusLine::default();
        line.set("正在加载推荐队列…".to_string());
        // 到点先进入退出动画，文案还在。
        assert!(line.visible(Instant::now() + VISIBLE_FOR).is_some());
        assert!(line.exiting);
        // 退出动画结束后才真正消失。
        assert_eq!(
            line.visible(Instant::now() + VISIBLE_FOR + motion::duration::STATUS_OUT),
            None
        );
    }

    #[test]
    fn status_survives_until_the_deadline() {
        let mut line = StatusLine::default();
        line.set("已退出登录".to_string());
        assert_eq!(
            line.visible(Instant::now() + (VISIBLE_FOR / 2)),
            Some("已退出登录")
        );
    }

    #[test]
    fn a_new_status_replaces_the_old_one_and_restarts_the_timer() {
        let mut line = StatusLine::default();
        line.set("音质已设为 无损，从下一首生效".to_string());
        let first = line.claim_timer(Instant::now());
        line.set("已开启详细日志".to_string());
        let second = line.claim_timer(Instant::now());
        let (first, second) = (first.expect("first claims"), second.expect("second claims"));
        assert_ne!(first.0, second.0, "新状态要重新认领一次到期计时");
        assert_eq!(line.visible(Instant::now()), Some("已开启详细日志"));
        // 旧文案留作交叉淡化。
        assert_eq!(
            line.previous.as_deref(),
            Some("音质已设为 无损，从下一首生效")
        );
    }

    #[test]
    fn only_the_latest_status_timer_can_expire_it() {
        let mut line = StatusLine::default();
        line.set("第一条".to_string());
        let (first, _) = line
            .claim_timer(Instant::now())
            .expect("first claims a timer");
        line.set("第二条".to_string());
        let (second, _) = line
            .claim_timer(Instant::now())
            .expect("second claims its own timer");
        assert!(!line.expire(first), "旧计时器不该收走新文案");
        assert!(line.expire(second));
        assert!(line.finish_exit(second));
        assert_eq!(line.visible(Instant::now()), None);
    }

    #[test]
    fn clearing_now_hides_the_status_for_good() {
        let mut line = StatusLine::default();
        line.set("正在搜索「晴天」…".to_string());
        let (seq, _) = line
            .claim_timer(Instant::now())
            .expect("status claims a timer");
        line.clear();
        assert_eq!(line.visible(Instant::now()), None);
        assert!(!line.expire(seq));
    }

    #[test]
    fn each_status_claims_exactly_one_timer() {
        let mut line = StatusLine::default();
        line.set("正在读取歌单…".to_string());
        assert!(line.claim_timer(Instant::now()).is_some());
        assert!(
            line.claim_timer(Instant::now()).is_none(),
            "同一条状态不该每帧再排一次"
        );
    }

    #[test]
    fn an_on_time_first_render_gets_the_whole_window() {
        let mut line = StatusLine::default();
        line.set("正在读取歌单…".to_string());
        let (_, remaining) = line
            .claim_timer(Instant::now())
            .expect("status claims a timer");
        assert!(
            remaining <= VISIBLE_FOR && remaining > VISIBLE_FOR - Duration::from_millis(100),
            "首帧准时应当排满整段窗口，实得 {remaining:?}"
        );
    }

    #[test]
    fn a_late_first_render_only_gets_the_remaining_time() {
        let mut line = StatusLine::default();
        line.set("正在加载歌单…".to_string());
        // 首次渲染晚了 4 秒：计时器只能排剩下的那 1 秒，不能重新排整段 5 秒。
        let now = Instant::now() + (VISIBLE_FOR * 4 / 5);
        let (_, remaining) = line.claim_timer(now).expect("status claims a timer");
        assert!(
            remaining <= VISIBLE_FOR / 5
                && remaining > VISIBLE_FOR / 5 - Duration::from_millis(100),
            "首帧来晚了应当只排剩余时长，实得 {remaining:?}"
        );
    }

    #[test]
    fn an_already_expired_status_claims_a_zero_timer() {
        // 渲染入口先过 `visible()`，正常路径走不到这里；真走到了就让计时器立刻到期，
        // 而不是再排一整段。
        let mut line = StatusLine::default();
        line.set("正在搜索…".to_string());
        let (_, remaining) = line
            .claim_timer(Instant::now() + VISIBLE_FOR)
            .expect("status claims a timer");
        assert_eq!(remaining, Duration::ZERO);
    }

    #[test]
    fn a_status_never_written_claims_no_timer() {
        let mut line = StatusLine::default();
        assert!(line.claim_timer(Instant::now()).is_none());
    }
}
