//! 列表滚轮的平滑滚动接入。
//!
//! 视图侧只做两件事：把列表的滚动句柄交给这里，并在滚轮事件里调用
//! `smooth_wheel`。所有缓动状态都挂在 `Root::scroll_anims` 上，逐帧推进，
//! 因此不会随重绘丢失。
//!
//! 「指针是否在这个列表上」直接用滚动句柄自己的 `bounds()` 判断：这正是
//! GPUI 在 prepaint 里写入的窗口坐标，和滚轮事件的 `position` 同一套坐标，
//! 不再额外维护一层 canvas 量出来的 bounds。

use gpui::prelude::*;
use gpui::{
    div, px, Div, ScrollHandle, ScrollWheelEvent, Stateful, UniformListScrollHandle, Window,
};
use std::time::Instant;

use crate::app::Root;
use crate::ui::smooth_scroll::{self, ScrollSlot};
use crate::ui::{player_bar, theme};
use crate::views::player::LYRIC_ROW_H;

/// 所有滚轮区域共用的监听器：只做「把事件交给 `Root::smooth_wheel`」这一件事。
///
/// 视图侧不要再自己写闭包，否则不同页面很容易出现“手感不一样”的分叉实现。
pub(crate) fn wheel_listener<'a>(
    cx: &'a mut gpui::Context<Root>,
    key: &'static str,
    handle: ScrollHandle,
) -> impl Fn(&ScrollWheelEvent, &mut Window, &mut gpui::App) + 'static + use<'a> {
    cx.listener(move |root, event, window, cx| {
        root.smooth_wheel(key, handle.clone(), event, window, cx);
    })
}

/// `uniform_list` 的滚轮监听器：取出底层 `ScrollHandle` 后走同一条路径。
pub(crate) fn uniform_wheel_listener<'a>(
    cx: &'a mut gpui::Context<Root>,
    key: &'static str,
    handle: UniformListScrollHandle,
) -> impl Fn(&ScrollWheelEvent, &mut Window, &mut gpui::App) + 'static + use<'a> {
    let base = handle.0.borrow().base_handle.clone();
    wheel_listener(cx, key, base)
}

/// 所有纵向滚动区域共用的外壳：id、滚动句柄、滚轮监听、隐藏滚动条、纵向滚动。
///
/// 侧栏/歌词等普通滚动容器都从这里出，布局差异在外层继续链式追加。
pub(crate) fn scroll_area(
    id: &'static str,
    cx: &mut gpui::Context<Root>,
    handle: ScrollHandle,
) -> Stateful<Div> {
    div()
        .id(id)
        .track_scroll(&handle)
        .on_scroll_wheel(wheel_listener(cx, id, handle))
        .scrollbar_width(px(0.0))
        .overflow_y_scroll()
}

impl Root {
    /// 队列侧板盖住的那块区域：侧板打开时，下面的主列表不能跟着滚。
    fn queue_covers_point(&self, window: &Window, position: gpui::Point<gpui::Pixels>) -> bool {
        if !(self.queue_open || self.queue_closing) {
            return false;
        }
        let viewport = window.viewport_size();
        position.x >= viewport.width - px(player_bar::QUEUE_WIDTH)
            && position.y <= viewport.height - px(theme::PLAYER_H)
    }

    /// 处理一次滚轮事件：攒目标值并启动逐帧缓动。
    pub(crate) fn smooth_wheel(
        &mut self,
        key: &'static str,
        handle: ScrollHandle,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if !handle.bounds().contains(&event.position) {
            return;
        }
        // 队列侧板是覆盖层：它自己的列表照常滚，下面被盖住的内容不响应。
        if key != "queue-drawer" && self.queue_covers_point(window, event.position) {
            return;
        }
        // 滚轮只属于指针所在的那个列表，不允许继续冒泡到后面的列表。
        cx.stop_propagation();
        // 用户自己翻歌词时暂停自动跟随，过几秒再回到当前行。
        if key == "lyrics-scroll" {
            self.lyrics_hold_until = Some(Instant::now() + std::time::Duration::from_secs(3));
            self.lyrics_scroll_line = None;
        }
        let line_height = window.line_height();
        let delta = f32::from(event.delta.pixel_delta(line_height).y);
        if delta == 0.0 {
            return;
        }
        let applied = smooth_scroll::read(&handle);
        let max = smooth_scroll::max_of(&handle);
        let slot = self.scroll_anims.entry(key).or_insert_with(|| ScrollSlot {
            anim: Default::default(),
            handle: handle.clone(),
        });
        slot.anim.wheel(applied, delta, max);
        smooth_scroll::apply(&slot.handle, slot.anim.current());
        if slot.anim.is_running() {
            self.ensure_scroll_loop(window, cx);
        }
        cx.notify();
    }

    /// 歌词跟随当前行居中。
    ///
    /// 锚点是**英文文本块的中心**，不是整行几何中心：译文显隐、行高变化都不会
    /// 让正在看的英文上下跳。内容顶部留了半个视口的空白，所以目标偏移就是
    /// `行累计高度 + 英文块中心`。和滚轮共用同一套弹簧，换行时连续滑动。
    pub(crate) fn tick_lyrics_center(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.player_view_open {
            self.lyrics_scroll_line = None;
            return;
        }
        let Some(index) = self.lyrics_active else {
            return;
        };
        // 用户刚用滚轮翻过歌词：先别抢回去，过几秒再重新跟随。
        if self
            .lyrics_hold_until
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let handle = self.lyrics_scroll.clone();
        let max = smooth_scroll::max_of(&handle);
        if max <= 0.0 && index > 0 {
            // 列表还没完成第一次 prepaint，等拿到真实高度再锚。
            return;
        }
        let target = self
            .lyrics_layout
            .as_ref()
            .and_then(|layout| layout.anchor_offset(index))
            // 排版缓存还没建好（首帧）：按旧固定行高估一个中心。
            .unwrap_or(index as f32 * LYRIC_ROW_H + LYRIC_ROW_H * 0.5)
            .min(max);
        let running = self
            .scroll_anims
            .get("lyrics-scroll")
            .is_some_and(|slot| slot.anim.is_running());
        let line_changed = self.lyrics_scroll_line != Some(index);
        let needs_nudge = (smooth_scroll::read(&handle) - target).abs() > 0.5;
        if !line_changed && (running || !needs_nudge) {
            return;
        }
        self.lyrics_scroll_line = Some(index);
        let slot = self
            .scroll_anims
            .entry("lyrics-scroll")
            .or_insert_with(|| ScrollSlot {
                anim: Default::default(),
                handle: handle.clone(),
            });
        slot.anim
            .animate_to_soft(smooth_scroll::read(&handle), target, max);
        smooth_scroll::apply(&slot.handle, slot.anim.current());
        self.ensure_scroll_loop(window, cx);
        cx.notify();
    }

    /// 外部跳转后让动画状态跟上（切歌单、滚到某首等）。
    pub(crate) fn sync_scroll_anims(&mut self) {
        for slot in self.scroll_anims.values_mut() {
            let applied = smooth_scroll::read(&slot.handle);
            let max = smooth_scroll::max_of(&slot.handle);
            slot.anim.resync(applied, max);
        }
        self.lyrics_scroll_line = None;
    }

    /// 启动逐帧缓动循环（已在跑就不重复挂）。
    pub(crate) fn ensure_scroll_loop(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.scroll_anim_loop {
            return;
        }
        self.scroll_anim_loop = true;
        cx.on_next_frame(window, |root, window, cx| {
            root.step_scroll_anims(window, cx)
        });
    }

    /// 推进一帧缓动。
    fn step_scroll_anims(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let now = Instant::now();
        let mut running = false;
        for slot in self.scroll_anims.values_mut() {
            if let Some(scrolled) = slot.anim.step(now) {
                smooth_scroll::apply(&slot.handle, scrolled);
                running = true;
            }
        }
        if running {
            cx.notify();
            cx.on_next_frame(window, |root, window, cx| {
                root.step_scroll_anims(window, cx)
            });
        } else {
            self.scroll_anim_loop = false;
        }
    }
}
