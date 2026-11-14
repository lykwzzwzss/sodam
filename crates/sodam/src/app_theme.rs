//! 封面色板的渐变过渡。
//!
//! 汽水音乐换歌/换歌单时，整窗背景、侧栏磨砂底、强调色是一起缓慢变化的，
//! 不是「啪」地跳到新封面的主题色。这里把新色板作为目标，用 OKLab 插值
//! 逐帧写回全局主题，过渡时长 800ms。
//!
//! 推进由渲染帧驱动（`tick_palette`），所以 240Hz 屏上就是 240 步过渡，
//! 不再被 16ms 定时器限制在 60fps。
//!
//! 没有封面/灰阶封面时，插值终点固定用 [`Palette::NEUTRAL_DARK`]，
//! 保证从彩色封面切过去也不会整页翻白。

use std::time::Instant;

use gpui::{Context, Window};
use sodam_core::artwork::Palette;

use crate::app::{PaletteAnim, Root};
use crate::ui::{motion, theme};

/// 整页色场的过渡时长：比其他内容慢一档，让「氛围最后稳定」。
const PALETTE_CROSSFADE: std::time::Duration = motion::duration::THEME;

impl Root {
    /// 平滑切换到新的封面主题色；`None` 表示回到中性主题。
    ///
    /// 若上一次渐变还没结束，会从**当前中间色**继续，不会跳回旧起点。
    pub(crate) fn animate_palette_to(&mut self, target: Option<Palette>, cx: &mut Context<Self>) {
        let neutral = Palette::NEUTRAL_DARK;
        let to = target.unwrap_or(neutral);
        let from_colorful = self.palette_current.colorful;
        let to_colorful = target.is_some_and(|palette| palette.colorful);

        // 两边都是灰阶/无封面：界面结构不变，直接落目标，别启动没有意义的渐变。
        if !from_colorful && !to_colorful {
            self.palette_anim = None;
            self.palette_current = to;
            theme::set_ambient_palette(target);
            cx.notify();
            return;
        }

        // 彩色 ↔ 灰阶：灰阶一侧统一用深色中性板，避免经过浅灰。
        let from = if from_colorful {
            self.palette_current
        } else {
            neutral
        };
        let to = if to_colorful { to } else { neutral };

        // 96px 和 320px 是同一张封面算出来的色板：数值会有细微差异，
        // 视觉上是同一个目标。这里直接并到当前过渡，避免完成后再播一次。
        if let Some(anim) = self.palette_anim.as_mut() {
            if palette_close(&anim.to, &to) {
                anim.target = target;
                return;
            }
        } else if palette_close(&self.palette_current, &to) {
            self.palette_current = to;
            theme::set_ambient_palette(target);
            cx.notify();
            return;
        }

        if self.palette_anim.is_none() && from == to {
            self.palette_current = to;
            theme::set_ambient_palette(target);
            return;
        }

        self.palette_anim = Some(PaletteAnim {
            from,
            to,
            target,
            started: Instant::now(),
        });
        theme::set_ambient_palette(Some(from));
        cx.notify();
    }

    /// 渲染帧调用：把渐变推进到当前时刻，必要时请求下一帧。
    pub(crate) fn tick_palette(&mut self, window: &mut Window) {
        let Some(anim) = self.palette_anim.as_ref() else {
            return;
        };
        let (from, to, target, started) = (anim.from, anim.to, anim.target, anim.started);
        let total = PALETTE_CROSSFADE.as_secs_f32().max(f32::EPSILON);
        let t = (started.elapsed().as_secs_f32() / total).clamp(0.0, 1.0);
        if t >= 1.0 {
            self.palette_current = to;
            self.palette_anim = None;
            theme::set_ambient_palette(target);
            return;
        }
        // ease-in-out cubic：两端都慢，颜色在中间段完成主要变化，
        // 收尾时已经接近目标，形成「氛围慢慢稳定下来」的观感。
        let eased = motion::ease_in_out_cubic(t);
        self.palette_current = Palette::lerp(from, to, eased);
        theme::set_ambient_palette(Some(self.palette_current));
        window.request_animation_frame();
    }
}

/// 两套色板是否在视觉上已经是同一个目标。
///
/// 96px 与 320px 来自同一张封面，聚类会有几个色阶的抖动；这里按最大
/// 通道差 10/255 视为相同，避免同一张图触发第二次 800ms 主题色过渡。
fn palette_close(left: &Palette, right: &Palette) -> bool {
    if left.colorful != right.colorful {
        return false;
    }
    let close = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b.iter()).all(|(a, b)| a.abs_diff(*b) <= 10);
    close(left.accent, right.accent)
        && close(left.dominant, right.dominant)
        && close(left.ambient, right.ambient)
        && left
            .base
            .iter()
            .zip(right.base.iter())
            .all(|(a, b)| close(*a, *b))
}
