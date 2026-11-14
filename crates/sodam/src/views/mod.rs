//! 右侧内容区路由与共享页面组件。

use gpui::prelude::*;
use gpui::{div, px, svg, uniform_list, AnimationExt, AnyElement, ClickEvent, Window};

use crate::app::Root;
use crate::ui::icons;
use crate::ui::motion;
use crate::ui::spinner::loading_state;
use crate::ui::theme;
use crate::ui::topbar;

pub mod local;
pub mod player;
pub mod settings;

/// 页面标题。
pub(crate) fn page_title(title: &str) -> AnyElement {
    div()
        .text_size(theme::Text::Title.size())
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::text())
        .child(title.to_string())
        .into_any_element()
}

/// 空状态：图标 + 一句说明（不要空白页）。
pub(crate) fn empty_state(icon: &'static str, text: &str) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(theme::space::MD))
        .py(px(theme::space::XXL))
        .child(
            svg()
                .path(icons::path(icon))
                .size(px(theme::ICON_XL))
                .text_color(theme::text_faint()),
        )
        .child(
            div()
                .text_size(theme::Text::Small.size())
                .text_color(theme::text_muted())
                .child(text.to_string()),
        )
        .into_any_element()
}

/// 渲染内容区（含顶栏）。
pub fn render(
    root: &mut Root,
    window: &Window,
    cx: &mut Context<Root>,
    own_titlebar: bool,
) -> impl IntoElement {
    let player_open = root.player_view_open;
    let settings_open = root.settings_open;
    let body = if player_open {
        player::player_view(root, window, cx)
    } else if settings_open {
        settings::settings_view(root, cx)
    } else {
        local::local_view(root, cx)
    };
    let page_key = if player_open {
        "player"
    } else if settings_open {
        "settings"
    } else {
        "local"
    };

    let mut content = div().flex().flex_col().flex_1().min_h(px(0.0)).w_full();
    if !player_open {
        content = content.p(px(theme::space::XL)).gap(px(theme::space::LG));
    }

    div()
        .flex()
        .flex_col()
        .size_full()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(topbar::render(root, window, cx, own_titlebar))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .relative()
                .with_animation(
                    gpui::ElementId::Name(format!("page-fade-{page_key}").into()),
                    motion::standard(motion::duration::PAGE),
                    |this, delta| this.opacity(delta).top(px(8.0 * (1.0 - delta))),
                )
                .child(content.child(body)),
        )
}
