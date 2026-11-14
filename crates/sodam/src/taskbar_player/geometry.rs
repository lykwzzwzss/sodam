//! 任务栏几何：占用区间 → 空闲区间 → 播放条落位。
//!
//! 全部是纯函数（没有 Win32、没有 GPUI），可单独测试。

use super::Rect;

/// 任务栏内部未被任何占用矩形覆盖的连续区间，从左到右，已合并重叠。
pub(crate) fn free_gaps(taskbar: Rect, occupied: &[Rect]) -> Vec<Rect> {
    let mut spans: Vec<(i32, i32)> = occupied
        .iter()
        .filter_map(|rect| rect.intersect(taskbar))
        .map(|rect| (rect.left, rect.right))
        .collect();
    spans.sort_unstable();

    let mut gaps = Vec::new();
    let mut cursor = taskbar.left;
    for (left, right) in spans {
        if left > cursor {
            gaps.push(Rect {
                left: cursor,
                top: taskbar.top,
                right: left,
                bottom: taskbar.bottom,
            });
        }
        cursor = cursor.max(right);
    }
    if cursor < taskbar.right {
        gaps.push(Rect {
            left: cursor,
            top: taskbar.top,
            right: taskbar.right,
            bottom: taskbar.bottom,
        });
    }
    gaps
}

/// 在最右侧「够宽」的空闲区间里落位。
///
/// 右缘与占用区留 `margin`（托盘/展开键的安全距离），上下各留 `inset`。
/// 宽度取 `min(desired, 区间可容纳宽度)`；所有区间都不够 `min` 时返回 `None`。
pub(crate) fn place(
    gaps: &[Rect],
    desired: i32,
    min: i32,
    inset: i32,
    margin: i32,
) -> Option<Rect> {
    for gap in gaps.iter().rev() {
        let width = desired.min(gap.width() - inset * 2 - margin);
        if width >= min {
            return Some(Rect {
                left: gap.right - inset - margin - width,
                top: gap.top + inset,
                right: gap.right - inset - margin,
                bottom: gap.bottom - inset,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{free_gaps, place, Rect};

    fn rect(left: i32, right: i32) -> Rect {
        Rect {
            left,
            top: 0,
            right,
            bottom: 60,
        }
    }

    fn taskbar() -> Rect {
        rect(0, 2560)
    }

    #[test]
    fn gaps_skip_occupied_spans_and_merge_overlaps() {
        let gaps = free_gaps(taskbar(), &[rect(0, 100), rect(90, 200), rect(1000, 1100)]);
        assert_eq!(gaps, vec![rect(200, 1000), rect(1100, 2560)]);
    }

    #[test]
    fn gaps_ignore_rects_outside_the_taskbar() {
        let gaps = free_gaps(taskbar(), &[rect(-50, -10), rect(2600, 2700)]);
        assert_eq!(gaps, vec![rect(0, 2560)]);
    }

    #[test]
    fn gaps_of_a_fully_occupied_taskbar_are_empty() {
        assert!(free_gaps(taskbar(), &[rect(0, 2560)]).is_empty());
        assert!(free_gaps(taskbar(), &[rect(-100, 3000)]).is_empty());
    }

    #[test]
    fn placement_takes_the_rightmost_gap_and_keeps_clear_of_the_tray() {
        let gaps = free_gaps(taskbar(), &[rect(0, 400), rect(2097, 2560)]);
        let bar = place(&gaps, 660, 390, 9, 51).expect("右边那块够宽");
        assert_eq!(bar.right, 2097 - 9 - 51);
        assert_eq!(bar.width(), 660);
        assert_eq!(bar.top, 9);
        assert_eq!(bar.bottom, 51);
    }

    #[test]
    fn placement_shrinks_to_the_gap_and_skips_narrow_ones() {
        // 最右侧只有 300px，不够 min=390；左边那块够宽，于是落到左边。
        let gaps = free_gaps(
            taskbar(),
            &[rect(0, 400), rect(1700, 2100), rect(2400, 2560)],
        );
        let bar = place(&gaps, 660, 390, 9, 51).expect("左边那块够宽");
        assert_eq!(bar.right, 1700 - 9 - 51);
        assert_eq!(bar.width(), 660);
    }

    #[test]
    fn placement_returns_none_when_nothing_fits() {
        let gaps = free_gaps(taskbar(), &[rect(0, 500), rect(800, 2560)]);
        assert!(place(&gaps, 660, 390, 9, 51).is_none());
    }
}
