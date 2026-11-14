//! 全局 message：Ant Design 风格的多条轻提示队列。
//!
//! 动效：新提示从右上方 6px 处淡入（200ms），到期/点击后 150ms 淡出再卸载；
//! 同 key 更新时卡片原位保留，只让文字做 140ms 交叉淡化，不再重复弹出。

use std::time::Duration;

use gpui::prelude::*;
use gpui::{div, px, AnimationExt, AnyElement, Context, ElementId};

use crate::app::Root;
use crate::ui::motion;
use crate::ui::theme;

/// 默认显示时长，与 Ant Design message 的默认值一致。
pub const DEFAULT_DURATION: Duration = Duration::from_secs(3);
/// 即使来自不同来源，也不让提示铺满整块界面。
const MAX_VISIBLE_MESSAGES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: u64,
    pub kind: MessageKind,
    pub text: String,
    /// `None` 表示不自动关闭；默认见 [`DEFAULT_DURATION`]。
    pub duration: Option<Duration>,
    /// 同一来源的提示会替换旧的，避免定时失败等重复事件堆成消息墙。
    pub key: Option<String>,
    /// 同 key 更新时递增；文字交叉淡化用，卡片本身不重播入场动画。
    pub revision: u64,
    /// 更新前的文字（只在淡化窗口内渲染）。
    pub previous_text: Option<String>,
    /// 正在播放退场动画，等 150ms 后真正卸载。
    pub closing: bool,
}

impl Message {
    fn accent(&self) -> gpui::Rgba {
        match self.kind {
            MessageKind::Warning => theme::warning(),
            MessageKind::Error => theme::danger(),
        }
    }

    fn icon(&self) -> &'static str {
        match self.kind {
            MessageKind::Warning => "!",
            MessageKind::Error => "×",
        }
    }
}

impl Root {
    /// 同 key 的消息替换旧消息；频率由调用方按事件类型控制。
    pub fn push_message_with_key(
        &mut self,
        kind: MessageKind,
        text: impl Into<String>,
        duration: Option<Duration>,
        key: Option<&str>,
    ) -> u64 {
        let id = upsert_message(
            &mut self.messages,
            &mut self.next_message_id,
            kind,
            text.into(),
            duration,
            key,
        );
        // 内容已更新：允许重新排一个到期计时（旧计时器靠 revision 自动失效）。
        self.message_timers.remove(&id);
        self.message_timers.retain(|message_id| {
            self.messages
                .iter()
                .any(|message| message.id == *message_id)
        });
        id
    }

    /// 立即移除（内部使用；用户关闭走 [`Root::dismiss_message`]）。
    pub fn close_message(&mut self, id: u64) -> bool {
        let before = self.messages.len();
        self.messages.retain(|message| message.id != id);
        self.messages.len() != before
    }

    /// 开始关闭：先播 150ms 淡出，再真正移除。
    pub(crate) fn dismiss_message(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(message) = self.messages.iter_mut().find(|message| message.id == id) else {
            return;
        };
        if message.closing {
            return;
        }
        message.closing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(motion::duration::TOAST_OUT)
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.close_message(id) {
                    root.message_timers.remove(&id);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 启动该条消息的计时器；消息被替换/关闭后计时器会安全退出。
    pub(crate) fn schedule_message_close(
        &mut self,
        id: u64,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self.messages.iter().find(|message| message.id == id) else {
            return;
        };
        let Some(duration) = message.duration else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |root, cx| {
                let current = root
                    .messages
                    .iter()
                    .any(|message| message.id == id && message.revision == revision);
                if current {
                    root.dismiss_message(id, cx);
                }
            });
        })
        .detach();
    }
}

fn upsert_message(
    messages: &mut Vec<Message>,
    next_id: &mut u64,
    kind: MessageKind,
    text: String,
    duration: Option<Duration>,
    key: Option<&str>,
) -> u64 {
    let key = key.map(str::to_string);
    if let Some(key) = key.as_deref() {
        if let Some(message) = messages
            .iter_mut()
            .find(|message| message.key.as_deref() == Some(key))
        {
            // 同 key：原位更新文字，保留卡片 id，文字交叉淡化而不是重新弹出。
            if message.text != text {
                message.previous_text = Some(std::mem::replace(&mut message.text, text));
            }
            message.kind = kind;
            message.duration = duration;
            message.revision = message.revision.wrapping_add(1);
            message.closing = false;
            return message.id;
        }
    }
    *next_id = next_id.wrapping_add(1);
    let id = *next_id;
    messages.push(Message {
        id,
        kind,
        text,
        duration,
        key,
        revision: 1,
        previous_text: None,
        closing: false,
    });
    if messages.len() > MAX_VISIBLE_MESSAGES {
        messages.drain(0..messages.len() - MAX_VISIBLE_MESSAGES);
    }
    id
}

/// 最高层浮层：多条消息按插入顺序堆叠，各自独立计时。
pub(crate) fn render(root: &mut Root, cx: &mut Context<Root>) -> Option<AnyElement> {
    if root.messages.is_empty() {
        return None;
    }
    let timers: Vec<(u64, u64)> = root
        .messages
        .iter()
        .filter_map(|message| message.duration.map(|_| (message.id, message.revision)))
        .collect();
    for (id, revision) in timers {
        if root.message_timers.insert(id) {
            root.schedule_message_close(id, revision, cx);
        }
    }
    root.message_timers
        .retain(|id| root.messages.iter().any(|message| message.id == *id));

    let messages = root.messages.clone();
    Some(
        gpui::deferred(
            div()
                .id("global-messages")
                .absolute()
                .top(px(48.0))
                .right(px(24.0))
                .flex()
                .flex_col()
                .items_end()
                .gap(px(theme::space::SM))
                .children(messages.into_iter().map(|message| {
                    let accent = message.accent();
                    let id = message.id;
                    let closing = message.closing;
                    let card = div()
                        .id(("message", id as usize))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(theme::space::SM))
                        .max_w(px(520.0))
                        .px(px(theme::space::LG))
                        .py(px(theme::space::SM))
                        .rounded(px(theme::radius::CARD))
                        .border_1()
                        .border_color(accent)
                        // 轻提示可能压在内容上：磨砂深底保证文字可读。
                        .bg(theme::glass_panel())
                        .backdrop_blur(px(20.0))
                        .text_color(theme::text())
                        .text_size(theme::Text::Small.size())
                        .shadow_lg()
                        .child(div().flex_none().text_color(accent).child(message.icon()))
                        .child({
                            let text_style = |text: String| -> AnyElement {
                                div().child(text).into_any_element()
                            };
                            let new = text_style(message.text.clone());
                            match message.previous_text.clone() {
                                // 同 key 更新：旧文字 140ms 淡出，新文字淡入。
                                Some(previous) => motion::crossfade_text(
                                    &format!("message-text-{id}"),
                                    message.revision,
                                    Some(text_style(previous)),
                                    new,
                                    motion::duration::FOCUS,
                                    2.0,
                                ),
                                None => new,
                            }
                        })
                        .on_click(cx.listener(move |root, _, _, cx| {
                            root.dismiss_message(id, cx);
                        }));
                    let card = if closing {
                        card.with_animation(
                            ElementId::Name(format!("message-out-{id}").into()),
                            motion::standard(motion::duration::TOAST_OUT),
                            |this, delta| this.opacity(1.0 - delta).top(px(-4.0 * delta)),
                        )
                        .into_any_element()
                    } else {
                        // 卡片 id 在消息生命周期内不变；入场动画只在首次挂载时播一次。
                        card.with_animation(
                            ElementId::Name(format!("message-in-{id}").into()),
                            motion::standard(motion::duration::TOAST_IN),
                            |this, delta| this.opacity(delta).top(px(-6.0 * (1.0 - delta))),
                        )
                        .into_any_element()
                    };
                    card
                })),
        )
        .with_priority(usize::MAX)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_message_uses_three_seconds() {
        assert_eq!(DEFAULT_DURATION, Duration::from_secs(3));
    }

    #[test]
    fn message_kinds_have_distinct_visual_tokens() {
        let warning = Message {
            id: 1,
            kind: MessageKind::Warning,
            text: "warning".into(),
            duration: None,
            key: None,
            revision: 1,
            previous_text: None,
            closing: false,
        };
        let error = Message {
            kind: MessageKind::Error,
            ..warning.clone()
        };
        assert_ne!(warning.icon(), error.icon());
        assert_ne!(warning.accent(), error.accent());
    }

    #[test]
    fn same_message_key_updates_in_place_and_keeps_revision_history() {
        let mut messages = Vec::new();
        let mut next_id = 0;
        let first = upsert_message(
            &mut messages,
            &mut next_id,
            MessageKind::Error,
            "same failure".into(),
            Some(DEFAULT_DURATION),
            Some("save-failed"),
        );
        let second = upsert_message(
            &mut messages,
            &mut next_id,
            MessageKind::Warning,
            "same failure, newer".into(),
            Some(DEFAULT_DURATION),
            Some("save-failed"),
        );
        // 同 key 保持同一个卡片 id：不重播入场动画，只做文字交叉淡化。
        assert_eq!(first, second);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, MessageKind::Warning);
        assert_eq!(messages[0].text, "same failure, newer");
        assert_eq!(messages[0].revision, 2);
        assert_eq!(messages[0].previous_text.as_deref(), Some("same failure"));
    }

    #[test]
    fn message_buffer_keeps_only_the_newest_entries() {
        let mut messages = Vec::new();
        let mut next_id = 0;
        for index in 0..=MAX_VISIBLE_MESSAGES {
            upsert_message(
                &mut messages,
                &mut next_id,
                MessageKind::Warning,
                format!("message {index}"),
                Some(DEFAULT_DURATION),
                None,
            );
        }
        assert_eq!(messages.len(), MAX_VISIBLE_MESSAGES);
        assert_eq!(messages[0].text, "message 1");
        assert_eq!(
            messages[MAX_VISIBLE_MESSAGES - 1].text,
            format!("message {MAX_VISIBLE_MESSAGES}")
        );
    }
}
