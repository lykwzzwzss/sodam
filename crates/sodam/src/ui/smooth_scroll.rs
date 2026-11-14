//! 滚轮平滑滚动。
//!
//! 平台滚轮事件是离散的（一格 = 3 行文字高度），直接把增量加到偏移上会
//! 「咯噔咯噔」地跳。这里把增量攒成目标值，再用逐帧缓动逼近：手停下后
//! 列表还会顺着惯性滑一小段，视觉上接近触控板 / 汽水音乐那种无极滚动。

use std::collections::HashMap;
use std::time::Instant;

use gpui::{point, px, ScrollHandle};

/// 目标吸引的刚度（1/s²）与阻尼（1/s）。临界阻尼约 `2*sqrt(k) ≈ 26.8`，
/// 这里略高于临界值，停下时不会来回晃。
const STIFFNESS: f32 = 180.0;
const DAMPING: f32 = 30.0;
/// 歌词换行居中的软弹簧：比滚轮更慢、接近临界阻尼，滑行时间约 0.6s，
/// 换行时是连续滑动而不是「啪」地弹到中间。
const LYRICS_STIFFNESS: f32 = 70.0;
const LYRICS_DAMPING: f32 = 17.0;
/// 速度上限，避免长按滚轮后瞬移。
const MAX_SPEED: f32 = 6_000.0;
/// 距离目标小于这个像素数、速度也足够小就贴合。
const SNAP_DISTANCE: f32 = 0.35;
const SNAP_SPEED: f32 = 6.0;
/// 滚轮一格给初速度的系数：手停下后还有一小段惯性。
const WHEEL_IMPULSE: f32 = 12.0;

/// 单个滚动区域的动画状态。
#[derive(Clone, Debug)]
pub struct ScrollAnim {
    /// 当前（渲染中）的滚动距离，0 = 顶部，正数 = 内容向上移。
    current: f32,
    /// 目标滚动距离。
    target: f32,
    /// 当前速度（像素/秒，正数向下）。
    velocity: f32,
    /// 可滚动的最大距离。
    max: f32,
    /// 是否还在缓动中。
    running: bool,
    /// 上一次物理步进的时间；按真实 dt 积分，帧率变化时手感一致。
    last_step: Option<Instant>,
    /// 当前这段缓动使用的弹簧参数（滚轮/普通锚定与歌词居中不同）。
    stiffness: f32,
    damping: f32,
}

impl Default for ScrollAnim {
    fn default() -> Self {
        Self {
            current: 0.0,
            target: 0.0,
            velocity: 0.0,
            max: 0.0,
            running: false,
            last_step: None,
            stiffness: STIFFNESS,
            damping: DAMPING,
        }
    }
}

impl ScrollAnim {
    /// 收到一格滚轮增量：`delta` 为正向（滚轮向上）时滚动距离减少。
    ///
    /// `applied` 是元素内建滚动已经落到的距离（`-offset.y`），
    /// 用来抵消内建的那一次瞬时跳动，并识别外部滚动（拖滚动条等）。
    pub fn wheel(&mut self, applied: f32, delta: f32, max: f32) {
        self.max = max.max(0.0);
        // 用户开始滚轮：立刻切回响应更快的标准弹簧。
        self.stiffness = STIFFNESS;
        self.damping = DAMPING;
        // 内建滚动可能已经先把这一格加到了 offset 上（取决于监听器顺序，
        // 我们又会 stop_propagation）。三种情况分开处理：
        // 1. applied 正好是「当前位置减增量」→ 内建滚动已生效，忽略它，
        //    调用方紧接着会把 offset 写回动画位置，抵消那一次跳动；
        // 2. applied 等于当前位置 → 内建滚动没生效，直接攒目标；
        // 3. 两者都不是 → 拖滚动条/代码跳转，先对齐实际位置。
        let builtin_applied = (applied - (self.current - delta)).abs() <= 1.0;
        if !builtin_applied && (applied - self.current).abs() > 1.0 {
            self.current = applied.clamp(0.0, self.max);
            self.target = self.current;
            self.velocity = 0.0;
        }
        self.target = (self.target - delta).clamp(0.0, self.max);
        // 给一个初速度，让滚轮停止后还有一小段惯性；不使用固定每帧插值，
        // 否则高刷屏上会显得「一格一格」。
        self.velocity = (self.velocity - delta * WHEEL_IMPULSE).clamp(-MAX_SPEED, MAX_SPEED);
        if (self.target - self.current).abs() < SNAP_DISTANCE && self.velocity.abs() < SNAP_SPEED {
            self.current = self.target;
            self.velocity = 0.0;
            self.running = false;
        } else {
            self.running = true;
            self.last_step = Some(Instant::now());
        }
    }

    /// 歌词换行居中：用更慢的软弹簧滑过去，换行时不会「啪」地跳到中间。
    ///
    /// 连续跳行时只换目标，不重置速度，所以快速连跳也是连续的滑行。
    pub fn animate_to_soft(&mut self, applied: f32, target: f32, max: f32) {
        self.animate_to_with(applied, target, max, LYRICS_STIFFNESS, LYRICS_DAMPING);
    }

    fn animate_to_with(
        &mut self,
        applied: f32,
        target: f32,
        max: f32,
        stiffness: f32,
        damping: f32,
    ) {
        self.max = max.max(0.0);
        self.stiffness = stiffness;
        self.damping = damping;
        self.current = applied.clamp(0.0, self.max);
        self.target = target.clamp(0.0, self.max);
        if (self.target - self.current).abs() < SNAP_DISTANCE && self.velocity.abs() < SNAP_SPEED {
            self.current = self.target;
            self.velocity = 0.0;
            self.running = false;
        } else {
            self.running = true;
            self.last_step = Some(Instant::now());
        }
    }

    /// 外部改了滚动位置：直接对齐，不做动画。
    pub fn resync(&mut self, applied: f32, max: f32) {
        self.max = max.max(0.0);
        self.current = applied.clamp(0.0, self.max);
        self.target = self.current;
        self.velocity = 0.0;
        self.running = false;
        self.last_step = None;
    }

    /// 推进一帧，返回本帧应当写入的滚动距离；已停稳则返回 `None`。
    pub fn step(&mut self, now: Instant) -> Option<f32> {
        if !self.running {
            return None;
        }
        let dt = self
            .last_step
            .map(|last| now.saturating_duration_since(last).as_secs_f32())
            .unwrap_or(1.0 / 60.0)
            .clamp(1.0 / 240.0, 1.0 / 30.0);
        self.last_step = Some(now);

        let displacement = self.target - self.current;
        let acceleration = displacement * self.stiffness - self.velocity * self.damping;
        self.velocity = (self.velocity + acceleration * dt).clamp(-MAX_SPEED, MAX_SPEED);
        self.current = (self.current + self.velocity * dt).clamp(0.0, self.max);

        // 撞到边界就停住，不让弹簧继续积累速度。
        if (self.current <= 0.0 && self.velocity < 0.0)
            || (self.current >= self.max && self.velocity > 0.0)
        {
            self.velocity = 0.0;
        }

        if (self.target - self.current).abs() <= SNAP_DISTANCE && self.velocity.abs() <= SNAP_SPEED
        {
            self.current = self.target;
            self.velocity = 0.0;
            self.running = false;
        }
        Some(self.current)
    }

    /// 当前滚动距离。
    pub fn current(&self) -> f32 {
        self.current
    }

    /// 是否还在缓动。
    pub fn is_running(&self) -> bool {
        self.running
    }
}

/// 一个滚动区域：动画状态 + 它的滚动句柄。
#[derive(Clone, Debug)]
pub struct ScrollSlot {
    /// 动画状态。
    pub anim: ScrollAnim,
    /// 元素的滚动句柄。
    pub handle: ScrollHandle,
}

/// 滚动区域的状态表：一个键对应一个列表。
pub type ScrollAnims = HashMap<&'static str, ScrollSlot>;

/// 把滚动距离写回元素的滚动句柄（offset 向下为负）。
pub fn apply(handle: &ScrollHandle, scrolled: f32) {
    handle.set_offset(point(px(0.0), px(-scrolled)));
}

/// 读元素当前的滚动距离。
pub fn read(handle: &ScrollHandle) -> f32 {
    -f32::from(handle.offset().y)
}

/// 读元素当前的最大滚动距离。
pub fn max_of(handle: &ScrollHandle) -> f32 {
    f32::from(handle.max_offset().y).max(0.0)
}
