//! 用封面多色色板绘制全局流动光斑。4 组光斑全部走 GPUI 原生高斯 SDF 着色，
//! 在任意刷新率下连续平滑轨道漂移，无离散帧跳变与闪烁。

use std::f32::consts::TAU;
use std::time::Instant;

use gpui::prelude::*;
use gpui::{canvas, div, point, px, size, Bounds, BoxShadow, Corners, IntoElement};

use crate::ui::theme;

#[derive(Default)]
pub(crate) struct FlowClock {
    phase: f32,
    last_frame: Option<Instant>,
}

impl FlowClock {
    pub(crate) fn advance(&mut self, now: Instant, running: bool) -> f32 {
        if running {
            if let Some(last) = self.last_frame.replace(now) {
                // 窗口恢复或切回前台时限制单步上限，避免光斑突然瞬移。
                let elapsed = now.saturating_duration_since(last).as_secs_f32().min(0.1);
                self.phase = (self.phase + elapsed * TAU / 36.0).rem_euclid(TAU);
            }
        } else {
            self.last_frame = None;
        }
        self.phase
    }
}

/// 全局四象限流动光斑的连续轨迹（整数倍频保证 `TAU` 周期零接缝循环）。
fn glow_centers(phase: f32) -> [(f32, f32); 4] {
    [
        (
            0.22 + 0.16 * phase.sin(),
            0.28 + 0.18 * (phase * 2.0 + 0.6).cos(),
        ),
        (
            0.76 + 0.17 * (phase * 2.0 + 1.1).sin(),
            0.26 - 0.16 * (phase + 0.8).cos(),
        ),
        (
            0.34 + 0.19 * (phase + 2.1).cos(),
            0.78 + 0.15 * (phase * 2.0 + 2.4).sin(),
        ),
        (
            0.72 + 0.18 * (phase * 2.0 + 3.7).cos(),
            0.68 - 0.18 * (phase + 3.2).sin(),
        ),
    ]
}

pub(crate) fn render(phase: f32) -> impl IntoElement {
    let flow = theme::flow_mix();
    let colors = theme::backdrop_glow_colors();
    let centers = glow_centers(phase);
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .overflow_hidden()
        .bg(theme::page_background())
        .when(flow > 0.001, |this| {
            this.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let short = bounds.size.width.min(bounds.size.height);
                        // 把锚点元素放到窗外（即使高 DPI 缩放也在裁剪区外），窗内只画纯高斯光斑。
                        let anchor = Bounds::new(
                            bounds.origin - point(px(24.0), px(24.0)),
                            size(px(2.0), px(2.0)),
                        );
                        for ((x, y), (color, radius_factor)) in centers
                            .into_iter()
                            .zip(colors.into_iter().zip([0.30, 0.32, 0.29, 0.27]))
                        {
                            if color.a <= 0.001 {
                                continue;
                            }
                            let radius = short * radius_factor;
                            let shadow = BoxShadow::new(
                                bounds.size.width * x + px(23.0),
                                bounds.size.height * y + px(23.0),
                                color.into(),
                            )
                            .spread_radius(radius)
                            .blur_radius(radius * 1.15);
                            window.paint_drop_shadows(anchor, Corners::all(radius), &[shadow]);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
        })
        .child(div().absolute().size_full().bg(theme::backdrop_scrim()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn flow_resumes_without_counting_paused_time() {
        let mut clock = FlowClock::default();
        let start = Instant::now();
        assert_eq!(clock.advance(start, true), 0.0);
        let playing = clock.advance(start + Duration::from_millis(20), true);
        assert!(playing > 0.0);
        assert_eq!(
            clock.advance(start + Duration::from_secs(1), false),
            playing
        );
        assert_eq!(clock.advance(start + Duration::from_secs(5), true), playing);
    }

    #[test]
    fn glow_centers_wrap_seamlessly_over_tau() {
        let start = glow_centers(0.0);
        let end = glow_centers(TAU);
        for (a, b) in start.iter().zip(end.iter()) {
            assert!((a.0 - b.0).abs() < 1e-5);
            assert!((a.1 - b.1).abs() < 1e-5);
        }
    }
}
