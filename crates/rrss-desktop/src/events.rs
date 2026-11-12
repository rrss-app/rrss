//! 把 `rrss-app` 的对话领域事件转成前端的 `chat:*` 事件。不做任何结算或写库。

use tauri::{AppHandle, Emitter};

use rrss_app::pi::{ChatEvent, ChatSink};

pub struct TauriTranslationSink {
    pub app: AppHandle,
}

impl rrss_app::translate::runtime::EventSink for TauriTranslationSink {
    fn emit(&self, event: rrss_app::translate::runtime::Event) {
        use rrss_app::views::TranslationEventKind::*;
        let payload = rrss_app::translate::ipc::event(event);
        let channel = match payload.kind {
            Start => "translate:start",
            Delta => "translate:delta",
            Done => "translate:done",
            Error => "translate:error",
        };
        if let Err(error) = self.app.emit(channel, payload) {
            tracing::warn!(%error, "emit translation event");
        }
    }
}

pub struct TauriChatSink {
    pub app: AppHandle,
}

impl ChatSink for TauriChatSink {
    fn emit(&self, event: ChatEvent) {
        let sent = match event {
            ChatEvent::Start(thread_id) => self.app.emit("chat:start", thread_id),
            ChatEvent::Delta(snapshot) => self.app.emit("chat:delta", snapshot),
            ChatEvent::Status(status) => self.app.emit("chat:status", status),
            ChatEvent::Confirm(request) => self.app.emit("chat:confirm", request),
            ChatEvent::Error(error) => self.app.emit("chat:error", error),
            ChatEvent::Context(context) => self.app.emit("chat:context", context),
            ChatEvent::Done(thread_id) => self.app.emit("chat:done", thread_id),
        };
        if let Err(err) = sent {
            tracing::warn!(error = %err, "emit chat event");
        }
    }
}
