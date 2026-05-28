//! [`ChannelGateway`] — the seam between channel ingress and the
//! agent loop.
//!
//! Sprint 5 of "Wire the Loop". A webhook arrives → channel
//! adapter HMAC-verifies it and produces a [`ChannelMessage`] → the
//! gateway runs that message through an [`AgentLoop`] → the gateway
//! POSTs the final assistant text back to the upstream through the
//! channel-matching [`OutboxTransport`].
//!
//! The gateway is intentionally narrow: it holds an [`AgentLoop`] and
//! a map of `channel_id → Arc<dyn OutboxTransport>`. Bins build it
//! once at startup and clone the `Arc<ChannelGateway>` into every
//! webhook handler.
//!
//! Retry / queueing / dead-letter handling are explicitly out of
//! scope here — a single ingress is one round-trip, and the daemon
//! plane on top of the gateway is responsible for back-pressure.

use std::collections::BTreeMap;
use std::sync::Arc;

use gaussclaw_agent::{AgentLoop, LoopOutcome, MemorySink, Message, Prompt, TurnError};

use crate::{ChannelMessage, OutboundMessage, OutboxTransport};

/// Errors a single ingress→agent→outbox round-trip can hit.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayError {
    /// The channel id from [`ChannelMessage::channel`] has no
    /// registered [`OutboxTransport`].
    #[error("no outbox transport registered for channel `{0}`")]
    UnknownChannel(String),
    /// The agent loop returned an error before producing any assistant
    /// text.
    #[error("agent loop: {0}")]
    Agent(String),
    /// The outbox transport rejected the assistant reply (non-2xx
    /// upstream, transport failure, etc.).
    #[error("outbox delivery: {0}")]
    Delivery(String),
}

/// Result alias for gateway round-trips.
pub type GatewayResult<T> = Result<T, GatewayError>;

/// Pairs an [`AgentLoop`] with a per-channel map of
/// [`OutboxTransport`]s. One ingress → one agent run → one delivery.
pub struct ChannelGateway {
    agent: Arc<AgentLoop>,
    outboxes: BTreeMap<String, Arc<dyn OutboxTransport>>,
    /// Optional prompt prefix prepended to every channel-ingress
    /// message so the agent knows the message arrived from a
    /// (potentially adversarial) channel surface and not from a
    /// trusted human at the console.
    system_prelude: Option<String>,
}

impl ChannelGateway {
    /// Build a gateway around an [`AgentLoop`].
    #[must_use]
    pub fn new(agent: Arc<AgentLoop>) -> Self {
        Self {
            agent,
            outboxes: BTreeMap::new(),
            system_prelude: None,
        }
    }

    /// Register a transport for the channel id it advertises via
    /// [`OutboxTransport::channel_id`]. Subsequent registrations for
    /// the same id overwrite the previous one.
    #[must_use]
    pub fn with_outbox(mut self, transport: Arc<dyn OutboxTransport>) -> Self {
        self.outboxes
            .insert(transport.channel_id().to_string(), transport);
        self
    }

    /// Set a system-prelude string the gateway prepends to every
    /// channel-ingress prompt. Typical use: tell the agent to keep
    /// replies short and surface-appropriate for the channel.
    #[must_use]
    pub fn with_system_prelude(mut self, prelude: impl Into<String>) -> Self {
        self.system_prelude = Some(prelude.into());
        self
    }

    /// Returns `true` iff an outbox is registered for `channel_id`.
    #[must_use]
    pub fn has_outbox(&self, channel_id: &str) -> bool {
        self.outboxes.contains_key(channel_id)
    }

    /// Channel ids with a wired outbox, lexicographic.
    #[must_use]
    pub fn channel_ids(&self) -> Vec<&str> {
        self.outboxes.keys().map(String::as_str).collect()
    }

    /// Run one round-trip: agent processes `msg.body` (with
    /// `msg.taint`), the final assistant text is POSTed back through
    /// the matching outbox, addressed to `msg.sender`.
    ///
    /// Returns the assistant text that was delivered (or attempted).
    /// On a partial failure — agent succeeded but delivery failed —
    /// returns [`GatewayError::Delivery`] without retrying.
    pub async fn dispatch_inbound(&self, msg: &ChannelMessage) -> GatewayResult<String> {
        let outbox = self
            .outboxes
            .get(&msg.channel)
            .cloned()
            .ok_or_else(|| GatewayError::UnknownChannel(msg.channel.clone()))?;

        // Build a prompt — channel surfaces get an optional system
        // prelude so the agent knows the request arrived over an
        // untrusted (or downgraded) channel, plus the user message.
        let mut messages = Vec::with_capacity(2);
        if let Some(prelude) = &self.system_prelude {
            messages.push(Message::new("system", prelude.clone()));
        }
        messages.push(Message::new("user", msg.body.clone()));
        let prompt = Prompt::new("default", messages);

        // MemorySink captures every LoopEvent and gives back the full
        // outcome — the gateway doesn't need to stream tokens since
        // the channel transport sends the reply in one shot.
        let sink = MemorySink::new();
        let outcome: LoopOutcome = self
            .agent
            .run(prompt, msg.taint, None, &sink)
            .await
            .map_err(|e: TurnError| GatewayError::Agent(format!("{e:?}")))?;

        let reply_text = outcome
            .assistants
            .last()
            .map(|c| c.text.clone())
            .unwrap_or_default();

        // Address the reply at the sender that produced the inbound
        // message. For chat-room channels (Slack #channel, Telegram
        // chat_id) the sender is the room; for direct-message channels
        // it's the user id.
        let outbound = OutboundMessage::new(msg.sender.clone(), reply_text.clone());
        outbox
            .deliver(&outbound)
            .await
            .map_err(|e| GatewayError::Delivery(e.to_string()))?;
        Ok(reply_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelError, ChannelResult, OutboxTransport};
    use async_trait::async_trait;
    use gauss_core::TaintLabel;
    use gaussclaw_agent::{AgentLoop, EchoProvider, KernelHandle, TurnPolicy};
    use std::sync::Mutex;

    /// Records every deliver() call so tests can assert wire shape.
    struct CapturingOutbox {
        id: String,
        delivered: Mutex<Vec<OutboundMessage>>,
        fail_next: Mutex<bool>,
    }

    impl CapturingOutbox {
        fn new(id: &str) -> Self {
            Self {
                id: id.into(),
                delivered: Mutex::new(Vec::new()),
                fail_next: Mutex::new(false),
            }
        }
        fn fail_next(&self) {
            *self.fail_next.lock().unwrap() = true;
        }
        fn snapshot(&self) -> Vec<OutboundMessage> {
            self.delivered.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl OutboxTransport for CapturingOutbox {
        fn channel_id(&self) -> &str {
            &self.id
        }
        async fn deliver(&self, msg: &OutboundMessage) -> ChannelResult<()> {
            if std::mem::replace(&mut *self.fail_next.lock().unwrap(), false) {
                return Err(ChannelError::Transport("forced".into()));
            }
            self.delivered.lock().unwrap().push(msg.clone());
            Ok(())
        }
    }

    fn echo_gateway(outbox: Arc<dyn OutboxTransport>) -> ChannelGateway {
        let provider = Arc::new(EchoProvider::default());
        let policy = TurnPolicy::new(KernelHandle::permissive(), provider);
        let agent = Arc::new(AgentLoop::new(policy));
        ChannelGateway::new(agent).with_outbox(outbox)
    }

    #[tokio::test]
    async fn round_trip_runs_agent_and_delivers_reply() {
        let outbox = Arc::new(CapturingOutbox::new("slack"));
        let gateway = echo_gateway(outbox.clone());
        let msg = ChannelMessage::new("slack", "#general", "hello there")
            .with_taint(TaintLabel::Web);
        let reply = gateway
            .dispatch_inbound(&msg)
            .await
            .expect("round trip");
        // EchoProvider repeats the prompt — assert the reply is
        // non-empty and the outbox received exactly one message
        // addressed at the sender.
        assert!(!reply.is_empty(), "reply must not be empty");
        let delivered = outbox.snapshot();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].recipient, "#general");
        assert_eq!(delivered[0].body, reply);
    }

    #[tokio::test]
    async fn unknown_channel_id_surfaces_as_error() {
        let outbox = Arc::new(CapturingOutbox::new("slack"));
        let gateway = echo_gateway(outbox);
        let msg = ChannelMessage::new("discord", "@x", "hi");
        let err = gateway.dispatch_inbound(&msg).await.expect_err("unknown");
        match err {
            GatewayError::UnknownChannel(id) => assert_eq!(id, "discord"),
            other => panic!("expected UnknownChannel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delivery_failure_propagates_after_agent_succeeds() {
        let outbox = Arc::new(CapturingOutbox::new("slack"));
        outbox.fail_next();
        let gateway = echo_gateway(outbox.clone());
        let msg = ChannelMessage::new("slack", "#g", "ping").with_taint(TaintLabel::Web);
        let err = gateway
            .dispatch_inbound(&msg)
            .await
            .expect_err("delivery failure");
        match err {
            GatewayError::Delivery(_) => {}
            other => panic!("expected Delivery, got {other:?}"),
        }
        // The outbox didn't capture the (failed) message.
        assert!(outbox.snapshot().is_empty());
    }

    #[tokio::test]
    async fn system_prelude_is_prepended_to_prompt_messages() {
        // Easiest way to assert the prelude reaches the agent: use a
        // scripted provider that echoes the *count* of messages back
        // so the reply tells us how many we built. We swap the
        // EchoProvider for a tiny mock provider that returns the
        // last system / user message text.
        use async_trait::async_trait;
        use gaussclaw_agent::{
            Completion, Prompt, ProviderHandle, ProviderResult, TokenCount,
        };

        struct CountingProvider;
        #[async_trait]
        impl ProviderHandle for CountingProvider {
            fn name(&self) -> &'static str {
                "counting"
            }
            async fn complete(&self, p: &Prompt) -> ProviderResult<Completion> {
                let count = p.messages.len();
                Ok(Completion::new(
                    format!("messages={count}"),
                    "counting",
                    "stop",
                    TokenCount::new(1, 1),
                ))
            }
        }

        let outbox: Arc<dyn OutboxTransport> = Arc::new(CapturingOutbox::new("slack"));
        let provider: Arc<dyn ProviderHandle> = Arc::new(CountingProvider);
        let policy = TurnPolicy::new(KernelHandle::permissive(), provider);
        let agent = Arc::new(AgentLoop::new(policy));
        let gateway = ChannelGateway::new(agent)
            .with_outbox(outbox.clone())
            .with_system_prelude("you're on a chat channel");

        let msg = ChannelMessage::new("slack", "#g", "hi").with_taint(TaintLabel::Web);
        let reply = gateway.dispatch_inbound(&msg).await.expect("ok");
        // With prelude: system + user → 2 messages.
        assert_eq!(reply, "messages=2");
    }

    /// Default (adversarial) taint is refused by the permissive kernel
    /// because no surface should silently accept adversarial input —
    /// callers must explicitly downgrade (typically after a signed
    /// webhook verify). This test locks that invariant in.
    #[tokio::test]
    async fn default_adversarial_taint_is_refused_by_permissive_kernel() {
        let outbox = Arc::new(CapturingOutbox::new("slack"));
        let gateway = echo_gateway(outbox.clone());
        let msg = ChannelMessage::new("slack", "#g", "hi");
        let err = gateway
            .dispatch_inbound(&msg)
            .await
            .expect_err("adversarial taint refused");
        match err {
            GatewayError::Agent(s) => assert!(s.contains("Denied")),
            other => panic!("expected Agent denial, got {other:?}"),
        }
        assert!(
            outbox.snapshot().is_empty(),
            "no reply should be delivered when the agent refused"
        );
    }

    #[test]
    fn has_outbox_reports_registered_channels() {
        let slack = Arc::new(CapturingOutbox::new("slack"));
        let discord = Arc::new(CapturingOutbox::new("discord"));
        let provider = Arc::new(EchoProvider::default());
        let policy = TurnPolicy::new(KernelHandle::permissive(), provider);
        let agent = Arc::new(AgentLoop::new(policy));
        let gateway = ChannelGateway::new(agent)
            .with_outbox(slack)
            .with_outbox(discord);
        assert!(gateway.has_outbox("slack"));
        assert!(gateway.has_outbox("discord"));
        assert!(!gateway.has_outbox("telegram"));
        let ids = gateway.channel_ids();
        assert_eq!(ids, vec!["discord", "slack"]);
    }
}
