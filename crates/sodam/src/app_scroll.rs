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
use std::time::{Duration, Instant};

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
            self.lyrics_scroll_stagger = None;
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
        if key == "settings-scroll" {
            self.settings_scroll_lock_until = None;
            self.sync_settings_section();
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
        let start = smooth_scroll::read(&handle);
        let needs_nudge = (start - target).abs() > 0.5;
        if !line_changed && (running || !needs_nudge) {
            return;
        }
        self.lyrics_scroll_line = Some(index);
        let now = Instant::now();
        let adjacent = self
            .lyrics_prev_active
            .is_some_and(|prev| (1..=4).contains(&index.saturating_sub(prev)));
        let normal_interval = self.lyrics.as_ref().is_some_and(|lyrics| {
            let at = lyrics.lines.get(index).and_then(|line| line.at_ms);
            let previous = index
                .checked_sub(1)
                .and_then(|index| lyrics.lines.get(index))
                .and_then(|line| line.at_ms);
            let next = lyrics.lines.get(index + 1).and_then(|line| line.at_ms);
            [previous.zip(at), at.zip(next)]
                .into_iter()
                .flatten()
                .all(|(start, end)| end.saturating_sub(start) >= 420)
        });
        self.lyrics_scroll_stagger = (line_changed
            && needs_nudge
            && self.settings.lyrics_staggered_scroll
            && adjacent
            && normal_interval
            && self
                .lyrics_switch_at
                .is_some_and(|at| now.duration_since(at) < Duration::from_millis(120))
            && !self
                .lyrics_last_seek
                .is_some_and(|at| now.duration_since(at) < Duration::from_millis(400)))
        .then_some((now, start));
        let slot = self
            .scroll_anims
            .entry("lyrics-scroll")
            .or_insert_with(|| ScrollSlot {
                anim: Default::default(),
                handle: handle.clone(),
            });
        slot.anim.animate_to_soft(start, target, max);
        smooth_scroll::apply(&slot.handle, slot.anim.current());
        window.request_animation_frame();
        cx.notify();
    }

    /// 设置页二级导航点击：平滑滚动到对应分组卡片。
    pub(crate) fn scroll_settings_to(
        &mut self,
        section_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_active_section = section_index;
        self.settings_scroll_lock_until = Some(Instant::now() + Duration::from_millis(650));
        let handle = self.settings_scroll.clone();
        let max = smooth_scroll::max_of(&handle);
        if max > 0.0 {
            if let (Some(first), Some(target_bounds)) = (
                handle.bounds_for_item(0),
                handle.bounds_for_item(section_index),
            ) {
                let target =
                    (f32::from(target_bounds.origin.y) - f32::from(first.origin.y)).clamp(0.0, max);
                let slot = self
                    .scroll_anims
                    .entry("settings-scroll")
                    .or_insert_with(|| ScrollSlot {
                        anim: Default::default(),
                        handle: handle.clone(),
                    });
                slot.anim
                    .animate_to_soft(smooth_scroll::read(&handle), target, max);
                smooth_scroll::apply(&slot.handle, slot.anim.current());
                window.request_animation_frame();
            }
        }
        cx.notify();
    }

    /// 手动滚动设置页时，自动把顶部二级导航高亮同步到当前可见的首个分组。
    pub(crate) fn sync_settings_section(&mut self) {
        if !self.settings_open || self.player_view_open {
            return;
        }
        if self
            .settings_scroll_lock_until
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let handle = &self.settings_scroll;
        let max = smooth_scroll::max_of(handle);
        if max <= 0.0 {
            return;
        }
        let scrolled = smooth_scroll::read(handle);
        if scrolled >= max - 4.0 {
            self.settings_active_section = 4;
            return;
        }
        let viewport_top = f32::from(handle.bounds().origin.y);
        let threshold = (f32::from(handle.bounds().size.height) * 0.22).clamp(96.0, 180.0);
        let mut active = 0usize;
        for idx in 0..5usize {
            let Some(bounds) = handle.bounds_for_item(idx) else {
                continue;
            };
            let visible_top = f32::from(bounds.origin.y) - scrolled - viewport_top;
            if visible_top <= threshold {
                active = idx;
            }
        }
        self.settings_active_section = active;
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

    /// 和窗口渲染共用同一个帧循环，避免重建窗口后遗留滚动回调状态。
    pub(crate) fn step_scroll_anims(&mut self, window: &mut Window) {
        let now = Instant::now();
        let mut running = false;
        let mut settings_moved = false;
        for (key, slot) in self.scroll_anims.iter_mut() {
            if let Some(scrolled) = slot.anim.step(now) {
                smooth_scroll::apply(&slot.handle, scrolled);
                running |= slot.anim.is_running();
                if *key == "settings-scroll" {
                    settings_moved = true;
                }
            }
        }
        if settings_moved {
            self.sync_settings_section();
        }
        if running {
            window.request_animation_frame();
        }
    }
}
