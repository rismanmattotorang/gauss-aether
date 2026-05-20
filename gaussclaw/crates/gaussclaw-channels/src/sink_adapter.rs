//! Adapter from the [`gaussclaw_tools::MessageSink`] trait to the
//! [`crate::ChannelRegistry`].
//!
//! Sprint 10 §5 of `/ROADMAP.md`. Sprint 9 §4 shipped the
//! `SendMessageTool` + `MessageSink` trait in
//! `gaussclaw-tools::sprint9_tools`. Production deployments need to
//! route those calls into the real channel adapters
//! ([`crate::SlackChannel`], [`crate::DiscordChannel`],
//! [`crate::TelegramChannel`], [`crate::EmailChannel`], the
//! webhook adapters in `sprint7_adapters`, etc.).
//!
//! This module is the bridge. `ChannelMessageSink::dispatch(channel,
//! recipient, body)` looks the channel up by id in the registry and
//! forwards the call as a typed [`crate::OutboundMessage`]. Unknown
//! channel ids surface as a typed error string the tool maps to
//! `GaussError::Internal("send: …")`.
//!
//! ## Hermes parity
//!
//! Hermes ships per-channel tool wrappers (`slack_send`, `discord_send`,
//! …) — adding a new channel means adding a new tool. The GaussClaw
//! approach keeps `send_message` as a single tool with a `{channel,
//! recipient, body}` arg shape; new channels register with the
//! `ChannelRegistry` and become available to the existing tool with no
//! code change.

#![allow(clippy::doc_markdown, clippy::module_name_repetitions)]

use std::sync::Arc;

use async_trait::async_trait;
use gaussclaw_tools::MessageSink;

use crate::{ChannelRegistry, OutboundMessage};

/// [`MessageSink`] implementation backed by a [`ChannelRegistry`].
///
/// Cheap to clone — internal state lives behind the `Arc<ChannelRegistry>`.
#[derive(Clone)]
pub struct ChannelMessageSink {
    registry: Arc<ChannelRegistry>,
}

impl ChannelMessageSink {
    /// Wrap a shared channel registry.
    #[must_use]
    pub const fn new(registry: Arc<ChannelRegistry>) -> Self {
        Self { registry }
    }

    /// Borrow the underlying registry — useful for registering more
    /// adapters at runtime.
    #[must_use]
    pub fn registry(&self) -> &Arc<ChannelRegistry> {
        &self.registry
    }
}

#[async_trait]
impl MessageSink for ChannelMessageSink {
    async fn dispatch(&self, channel: &str, recipient: &str, body: &str) -> Result<(), String> {
        self.registry
            .send(channel, OutboundMessage::new(recipient, body))
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelResult, ChannelTrait};
    use async_trait::async_trait;
    use gauss_core::CapToken;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    /// Counts every call to `send` — used to prove the bridge actually
    /// routes through `ChannelTrait::send`, not just the registry's
    /// lookup table.
    struct CountingChannel {
        id: String,
        count: AtomicUsize,
    }

    impl CountingChannel {
        fn new(id: impl Into<String>) -> Self {
            Self {
                id: id.into(),
                count: AtomicUsize::new(0),
            }
        }
        fn count(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ChannelTrait for CountingChannel {
        fn id(&self) -> &str {
            &self.id
        }
        fn required_caps(&self) -> CapToken {
            CapToken::NETWORK_POST
        }
        async fn send(&self, _msg: OutboundMessage) -> ChannelResult<()> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Captures every `OutboundMessage` for assertions on recipient /
    /// body / options.
    struct RecordingChannel {
        id: String,
        seen: StdMutex<Vec<OutboundMessage>>,
    }

    impl RecordingChannel {
        fn new(id: impl Into<String>) -> Self {
            Self {
                id: id.into(),
                seen: StdMutex::new(Vec::new()),
            }
        }
        fn seen(&self) -> Vec<OutboundMessage> {
            self.seen.lock().expect("poisoned").clone()
        }
    }

    #[async_trait]
    impl ChannelTrait for RecordingChannel {
        fn id(&self) -> &str {
            &self.id
        }
        fn required_caps(&self) -> CapToken {
            CapToken::NETWORK_POST
        }
        async fn send(&self, msg: OutboundMessage) -> ChannelResult<()> {
            self.seen.lock().expect("poisoned").push(msg);
            Ok(())
        }
    }

    #[tokio::test]
    async fn dispatch_routes_to_registered_channel() {
        let registry = Arc::new(ChannelRegistry::new());
        let slack = Arc::new(CountingChannel::new("slack"));
        registry.register(slack.clone()).await;
        let sink = ChannelMessageSink::new(registry);
        sink.dispatch("slack", "#general", "hello").await.unwrap();
        assert_eq!(slack.count(), 1);
    }

    #[tokio::test]
    async fn dispatch_forwards_recipient_and_body_to_outbound_message() {
        let registry = Arc::new(ChannelRegistry::new());
        let recorder = Arc::new(RecordingChannel::new("test"));
        registry.register(recorder.clone()).await;
        let sink = ChannelMessageSink::new(registry);
        sink.dispatch("test", "alice", "hi alice").await.unwrap();
        let seen = recorder.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].recipient, "alice");
        assert_eq!(seen[0].body, "hi alice");
    }

    #[tokio::test]
    async fn unknown_channel_surfaces_as_typed_error_string() {
        let registry = Arc::new(ChannelRegistry::new());
        let sink = ChannelMessageSink::new(registry);
        let err = sink
            .dispatch("nonexistent", "x", "y")
            .await
            .expect_err("should reject unknown channel");
        assert!(err.contains("nonexistent"));
    }

    #[tokio::test]
    async fn dispatch_routes_to_correct_channel_when_multiple_registered() {
        let registry = Arc::new(ChannelRegistry::new());
        let slack = Arc::new(CountingChannel::new("slack"));
        let discord = Arc::new(CountingChannel::new("discord"));
        let telegram = Arc::new(CountingChannel::new("telegram"));
        registry.register(slack.clone()).await;
        registry.register(discord.clone()).await;
        registry.register(telegram.clone()).await;
        let sink = ChannelMessageSink::new(registry);
        sink.dispatch("discord", "#x", "y").await.unwrap();
        sink.dispatch("discord", "#x", "y").await.unwrap();
        sink.dispatch("slack", "#x", "y").await.unwrap();
        assert_eq!(slack.count(), 1);
        assert_eq!(discord.count(), 2);
        assert_eq!(telegram.count(), 0);
    }

    /// End-to-end: drive the bridge through the actual
    /// `SendMessageTool` from `gaussclaw-tools`. Proves the wiring is
    /// usable by the tool layer without any further adapter code.
    #[tokio::test]
    async fn send_message_tool_dispatches_through_the_bridge() {
        use gauss_traits::ToolTrait;
        use gaussclaw_tools::SendMessageTool;

        let registry = Arc::new(ChannelRegistry::new());
        let counter = Arc::new(CountingChannel::new("slack"));
        registry.register(counter.clone()).await;
        let sink: Arc<dyn MessageSink> = Arc::new(ChannelMessageSink::new(registry));
        let tool = SendMessageTool::new(sink);
        let out = tool
            .invoke_raw(serde_json::json!({
                "channel": "slack",
                "recipient": "#general",
                "body": "from the tool"
            }))
            .await
            .expect("tool dispatch");
        assert_eq!(out["kind"], "message_queued");
        assert_eq!(counter.count(), 1);
    }
}
