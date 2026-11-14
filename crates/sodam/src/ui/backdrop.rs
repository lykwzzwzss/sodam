//! 全窗背景：由当前封面 9 阶色板铺成的整页色场。
//!
//! 不是「把专辑封面高斯模糊铺在底下」：汽水音乐切换歌曲时，变化的是
//! **整页的颜色**——页面、侧栏、播放栏一起从上一套色板过渡到下一套。
//! 这里只画颜色层，侧栏/播放栏再用半透明底 + `backdrop_blur` 做成磨砂玻璃，
//! 让整页颜色透上来。

use gpui::prelude::*;
use gpui::{div, px, Background, IntoElement};

use crate::ui::theme;

pub(crate) fn render() -> impl IntoElement {
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .overflow_hidden()
        // 底色：base-5 → base-7 的纵向渐变。
        .bg(theme::page_background())
        // 封面环境色：左下角更深的一团，和强调色错开。
        .child(fill(theme::backdrop_ambient_glow()))
        // 封面活力色：右上角的柔光，整页最容易被认出的那层颜色。
        .child(fill(theme::backdrop_glow()))
        // 上下暗化：保证顶栏与播放栏文字有足够对比。
        .child(fill(theme::backdrop_scrim()))
}

fn fill(background: Background) -> impl IntoElement {
    div()
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .bg(background)
}
