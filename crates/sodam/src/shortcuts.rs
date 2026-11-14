//! 播放控制快捷键：`Space` 加一对「上一首 / 下一首」。
//!
//! 默认值是分平台的（见 `sodam_core::config` 的注释）：macOS 用 `⌘←`/`⌘→`，
//! 因为 `⌃←`/`⌃→` 被系统「切换桌面」抢先吃掉；其他平台用 `Ctrl+←`/`Ctrl+→`。
//! 配置里仍然可以手改组合键，启动时由 [`bindings`] 读取。
//!
//! 绑定写在 `!input` 上下文上：搜索框（见 `app_text`）打字时让路。
//! 系统媒体键（媒体面板 / 播放器控件）走 `media` 模块，和这里共用同一组动作。

use gpui::{actions, Action, App, Entity, KeyBinding, Keystroke};

use crate::app::Root;
use sodam_core::Settings;

actions!(sodam, [TogglePlayback, SongPrevious, SongNext]);

/// 窗口根元素挂的基础上下文。
///
/// 必须有它：gpui 的上下文谓词在**空**上下文栈上恒为 false（`eval_inner` 里
/// `contexts.last()` 为空就直接返回 false），所以带谓词的绑定在「没有元素聚焦」
/// 时根本匹配不上。根元素挂上基础上下文后，栈至少有一层。
pub const BASE_CONTEXT: &str = "workspace";
/// 搜索框聚焦时挂的上下文；编辑键只在这里生效。
pub const INPUT_CONTEXT: &str = "input";

/// 播放动作与「聚焦搜索框」共同的生效条件：搜索框聚焦时让路。
pub(crate) const GLOBAL_CONTEXT: &str = "workspace && !input";

/// 「聚焦搜索框」的绑定串（见 `app_text::focus_bindings`）。
///
/// `secondary-` 由 gpui 按平台解释成 macOS 的 ⌘ / 其他平台的 Ctrl。
pub const FOCUS_SEARCH_SOURCE: &str = "secondary-f";

/// 这个串能不能当应用内快捷键绑定。
///
/// 三道关：非空、纯 ASCII、每个组合都能被 `Keystroke::parse` 接受。ASCII 这一条专门用来
/// 挡住 `⌘←` / `⊞k` 这种符号形式——它们 parse 不成修饰键，却会被 gpui 当成一个名叫
/// `⌘←` 的键，于是静默永不触发；`KeyBinding::new` 又是在解析失败时 panic 的入口
/// （`keymap/binding.rs` 的文档明写），配置文件用户可以手改，崩在启动上违反
/// AGENTS.md 的「错误不 panic」，所以坏值一律走这里挡掉。
pub fn is_bindable_source(source: &str) -> bool {
    let source = source.trim();
    !source.is_empty()
        && source.is_ascii()
        && source
            .split_whitespace()
            .all(|part| Keystroke::parse(part).is_ok())
}

/// 装好动作处理器，并按配置应用一次绑定。启动时调用一次。
pub fn install(cx: &mut App, settings: &Settings, root: Entity<Root>) {
    cx.on_action({
        let root = root.clone();
        move |_: &TogglePlayback, cx: &mut App| {
            root.update(cx, |root, cx| root.toggle_play(cx));
        }
    });
    cx.on_action({
        let root = root.clone();
        move |_: &SongPrevious, cx: &mut App| {
            root.update(cx, |root, cx| root.prev_track(cx));
        }
    });
    cx.on_action({
        let root = root.clone();
        move |_: &SongNext, cx: &mut App| {
            root.update(cx, |root, cx| root.next_track(cx));
        }
    });

    apply(cx, settings);
}

/// 重新应用全部绑定。
///
/// 整表重建而不是增量添加：gpui 的 keymap 是累加的，只加新绑定的话老键位会继续生效。
pub fn apply(cx: &mut App, settings: &Settings) {
    cx.clear_key_bindings();
    #[cfg(target_os = "macos")]
    cx.bind_keys(crate::menus::window_bindings());
    cx.bind_keys(bindings(settings));
    // 搜索框的编辑键单独一份绑定。
    cx.bind_keys(crate::app_text::bindings());
    cx.bind_keys(crate::app_text::focus_bindings());
}

/// 按配置生成三个播放动作的绑定。
///
/// 坏值只丢它自己那一条，另外两条照常生效：`KeyBinding::new` 解析失败会 panic，
/// 而这个字符串来自用户可以手改的 `config.json`。
fn bindings(settings: &Settings) -> Vec<KeyBinding> {
    let mut bindings = Vec::with_capacity(3);
    push_binding(&mut bindings, settings.shortcut_toggle(), TogglePlayback);
    push_binding(&mut bindings, settings.shortcut_previous(), SongPrevious);
    push_binding(&mut bindings, settings.shortcut_next(), SongNext);
    bindings
}

fn push_binding<A: Action>(bindings: &mut Vec<KeyBinding>, source: &str, action: A) {
    if !is_bindable_source(source) {
        return;
    }
    bindings.push(KeyBinding::new(source.trim(), action, Some(GLOBAL_CONTEXT)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 符号形式（旧版在 macOS / Windows 上存下来的样子）不可绑定，也不该进 keymap。
    #[test]
    fn symbol_forms_are_rejected() {
        assert!(!is_bindable_source("^←"));
        assert!(!is_bindable_source("⌘←"));
        assert!(!is_bindable_source("⊞k"));
        assert!(!is_bindable_source("   "), "空白等于未设置");
        assert!(!is_bindable_source(""));
        assert!(is_bindable_source("ctrl-a"));
        assert!(is_bindable_source("secondary-left"));
    }

    /// 一条坏值只作废它自己：另外两个动作照常绑上，且全程不 panic。
    #[test]
    fn one_bad_value_drops_only_its_own_binding() {
        let settings = Settings {
            shortcut_toggle: String::new(),
            shortcut_previous: "⌘←".into(),
            shortcut_next: String::new(),
            ..Default::default()
        };
        assert_eq!(
            bindings(&settings).len(),
            2,
            "坏掉的上一首被跳过，播放/暂停与下一首仍在"
        );

        let all_bad = Settings {
            shortcut_toggle: "⌘Q".into(),
            shortcut_previous: "^←".into(),
            shortcut_next: "⊞".into(),
            ..Default::default()
        };
        assert!(bindings(&all_bad).is_empty());
    }
}
