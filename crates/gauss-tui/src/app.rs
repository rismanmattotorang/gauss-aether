//! `gauss-tui` application state.
//!
//! The TUI binds against the live Gauss-Aether crate surface — every panel
//! reads (and a few mutate) real state through `Arc`-shared kernel /
//! memory / SAG / health handles. The mock starter data is just *seed*
//! values; once the user accepts an approval, runs a turn, or routes a
//! session, the state updates are real.

use std::collections::VecDeque;
use std::sync::Arc;

use gauss_audit::{Ed25519Signer, ReceiptSigner};
use gauss_bench::{gauss_aether_one_point_zero, predecessor_baselines, Scorecard};
use gauss_canvas::{Canvas, CanvasNode, CanvasUpdate, InMemoryCanvas, NodeId, WidgetKind};
use gauss_core::{Action, CapToken, Observation, ObservationSource, TaintLabel, TurnId};
use gauss_health::{HealthEngine, HealthReport, MockPresence, MockSubject};
use gauss_kernel::{ConsistentHashRing, NodeId as ClusterNodeId, PrivilegedKernel};
use gauss_memory::SurrealMemory;
use gauss_provider::ToyProvider;
use gauss_sag::{
    default_decision_table, ApprovalDecision, ApprovalGate, ApprovalRequest, AutoApprove,
    ChannelSurface,
};
use gauss_traits::{Kernel, MemoryBackend};
use gauss_turn::{DynSigningBackend, SagDecisionRecord, TurnEngine, TurnInput, TurnSummary};
use tokio::sync::{mpsc, Mutex};
use tokio::time::Instant;

/// The ten tabs.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Tab {
    Dashboard,
    Turns,
    Memory,
    Sandbox,
    Sag,
    Health,
    Cluster,
    Audit,
    Scorecard,
    Logs,
}

impl Tab {
    pub const fn all() -> [Self; 10] {
        [
            Self::Dashboard,
            Self::Turns,
            Self::Memory,
            Self::Sandbox,
            Self::Sag,
            Self::Health,
            Self::Cluster,
            Self::Audit,
            Self::Scorecard,
            Self::Logs,
        ]
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Turns => "Turns",
            Self::Memory => "Memory",
            Self::Sandbox => "Sandbox",
            Self::Sag => "SAG",
            Self::Health => "Health",
            Self::Cluster => "Cluster",
            Self::Audit => "Audit",
            Self::Scorecard => "Scorecard",
            Self::Logs => "Logs",
        }
    }

    pub const fn shortcut(self) -> char {
        match self {
            Self::Dashboard => '1',
            Self::Turns => '2',
            Self::Memory => '3',
            Self::Sandbox => '4',
            Self::Sag => '5',
            Self::Health => '6',
            Self::Cluster => '7',
            Self::Audit => '8',
            Self::Scorecard => '9',
            Self::Logs => '0',
        }
    }

    pub fn index(self) -> usize {
        Self::all().iter().position(|t| *t == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> Self {
        Self::all()[i.min(Self::all().len() - 1)]
    }

    pub fn next(self) -> Self {
        let n = Self::all().len();
        Self::from_index((self.index() + 1) % n)
    }

    pub fn prev(self) -> Self {
        let n = Self::all().len();
        Self::from_index((self.index() + n - 1) % n)
    }
}

/// One historical entry for the Turns tab.
#[derive(Debug, Clone)]
pub struct TurnHistoryEntry {
    pub id: TurnId,
    pub action_count: usize,
    pub chain_length: u64,
    pub chain_head_hex: String,
    pub taint: TaintLabel,
    pub committed_at_ms: u128,
    pub sag_decisions: Vec<SagDecisionRecord>,
}

impl TurnHistoryEntry {
    fn from_summary(summary: &TurnSummary, taint: TaintLabel) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        Self {
            id: summary.id,
            action_count: summary.action_count,
            chain_length: summary.chain_head.length,
            chain_head_hex: hex::encode(summary.chain_head.digest),
            taint,
            committed_at_ms: now_ms,
            sag_decisions: summary.sag_decisions.clone(),
        }
    }
}

/// One pending log line in the Logs tab.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: &'static str,
    pub message: String,
    pub ts_ms: u128,
}

/// One pending approval — shown in the SAG tab.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub received_at: Instant,
}

/// The full application state.
pub struct App {
    /// Currently visible tab.
    pub tab: Tab,
    /// Help overlay visible.
    pub show_help: bool,
    /// Should the event loop terminate?
    pub should_quit: bool,
    /// Last refresh of the polling-derived stats.
    pub last_refresh: Instant,
    /// User-visible flash message (banner at the bottom).
    pub flash: Option<(String, &'static str)>,

    // --- Live state ----------------------------------------------------
    /// The engine.
    pub engine: Arc<TurnEngine<PrivilegedKernel, SurrealMemory, ToyProvider>>,
    /// Shared kernel.
    pub kernel: Arc<PrivilegedKernel>,
    /// Shared memory backend.
    pub memory: Arc<SurrealMemory>,
    /// Health engine.
    pub health: HealthEngine,
    /// Last health report (re-evaluated on `r`).
    pub last_health: HealthReport,
    /// Mock health subject; the TUI drives this from the live state.
    pub health_subject: MockSubject,
    /// Consistent-hash cluster ring.
    pub ring: Arc<ConsistentHashRing>,
    /// Scorecards.
    pub me_scorecard: Scorecard,
    pub predecessor_scorecards: [Scorecard; 4],
    pub scorecard_focus: usize,
    /// Pending approvals received from the SAG `ChannelSurface`.
    pub pending: Arc<Mutex<VecDeque<PendingApproval>>>,
    /// Sender that pushes operator decisions back into the surface.
    pub decision_tx: mpsc::Sender<ApprovalDecision>,
    /// Highlighted index in the pending list.
    pub pending_cursor: usize,
    /// Historical turns (most recent first).
    pub turn_history: VecDeque<TurnHistoryEntry>,
    /// Highlighted turn index.
    pub turn_cursor: usize,
    /// Live canvas — mirrors the current Dashboard widget tree.
    pub canvas: Arc<InMemoryCanvas>,
    /// Log buffer.
    pub logs: VecDeque<LogLine>,
    /// Turn counter (drives `TurnId`).
    pub next_turn: u128,
    /// Default observation taint for `r → run turn`.
    pub default_taint: TaintLabel,
    /// Cluster: hand-entered "test session" key.
    pub cluster_test_key: String,
    pub cluster_test_node: Option<String>,
}

impl App {
    /// Build the application state. Spawns a background task that drains
    /// the SAG approval-surface receiver into the [`Self::pending`]
    /// queue.
    pub async fn boot() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Memory.
        let memory = Arc::new(SurrealMemory::open_in_memory().await?);

        // 2. Kernel.
        let kernel = Arc::new(PrivilegedKernel::new(CapToken::TOP));

        // 3. Provider — toy provider that emits one text action per turn.
        let provider = Arc::new(ToyProvider::always_text("hello from gauss-tui"));

        // 4. SAG channel surface — we keep the receiver locally so the
        //    TUI can drain pending requests and post decisions back.
        let (channel_surface, mut req_rx) = ChannelSurface::new(64);
        let decision_tx = channel_surface.sender();
        let pending: Arc<Mutex<VecDeque<PendingApproval>>> = Arc::new(Mutex::new(VecDeque::new()));

        // Background drain task — forwards every incoming approval
        // request into the visible queue. The TUI's SAG view pops items
        // and sends the human verdict back over `decision_tx`.
        {
            let pending = Arc::clone(&pending);
            tokio::spawn(async move {
                while let Some(request) = req_rx.recv().await {
                    let mut q = pending.lock().await;
                    q.push_back(PendingApproval {
                        request,
                        received_at: Instant::now(),
                    });
                }
            });
        }

        let gate = Arc::new(ApprovalGate::new(default_decision_table(), channel_surface));

        // 5. Signer.
        let signer_inner = Ed25519Signer::from_seed([0x42_u8; 32]);
        let signer = Arc::new(ReceiptSigner::<DynSigningBackend>::new(
            DynSigningBackend::new(signer_inner),
        ));

        // 6. Engine.
        let engine = Arc::new(
            TurnEngine::with_signing(Arc::clone(&kernel), Arc::clone(&memory), provider, signer)
                .with_sag(gate),
        );

        // 7. Health.
        let health = HealthEngine::default();
        let health_subject = MockSubject {
            chain: Some((0, [0u8; 32])),
            grant: kernel.current_grant().bits(),
            live_workers: 0,
            presence: MockPresence::default(),
        };
        let last_health = health.evaluate(&health_subject);

        // 8. Cluster ring with three demo nodes.
        let ring = Arc::new(ConsistentHashRing::default());
        for n in ["gauss-1.eu-west", "gauss-2.eu-west", "gauss-3.us-east"] {
            ring.add_node(ClusterNodeId::new(n));
        }

        // 9. Scorecards.
        let me_scorecard = gauss_aether_one_point_zero();
        let predecessor_scorecards = predecessor_baselines();

        // 10. Canvas — seed one root + the live dashboard banner.
        let canvas = Arc::new(InMemoryCanvas::default());
        canvas
            .apply(CanvasUpdate::Insert {
                node: CanvasNode::leaf(
                    NodeId::new("dashboard-banner"),
                    WidgetKind::Markdown,
                    serde_json::json!({
                        "body": "**Gauss-Aether 1.0** · TUI Admin Console"
                    }),
                ),
                parent: None,
            })
            .await?;

        // 11. Pre-seeded logs.
        let mut logs = VecDeque::with_capacity(256);
        logs.push_back(LogLine {
            level: "INFO",
            message: "Gauss-Aether TUI booted — engine is live.".into(),
            ts_ms: now_ms(),
        });
        logs.push_back(LogLine {
            level: "INFO",
            message: format!(
                "Three cluster nodes wired: {}",
                ring.nodes()
                    .iter()
                    .map(|n| n.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            ts_ms: now_ms(),
        });

        Ok(Self {
            tab: Tab::Dashboard,
            show_help: false,
            should_quit: false,
            last_refresh: Instant::now(),
            flash: None,
            engine,
            kernel,
            memory,
            health,
            last_health,
            health_subject,
            ring,
            me_scorecard,
            predecessor_scorecards,
            scorecard_focus: 0,
            pending,
            decision_tx,
            pending_cursor: 0,
            turn_history: VecDeque::with_capacity(256),
            turn_cursor: 0,
            canvas,
            logs,
            next_turn: 1,
            default_taint: TaintLabel::User,
            cluster_test_key: "session-demo".into(),
            cluster_test_node: None,
        })
    }

    /// Refresh polling-driven stats. Called on `r` and on the 4-Hz tick.
    pub async fn refresh(&mut self) {
        // Pull the latest chain head.
        if let Ok(head) = self.memory.chain_head().await {
            self.health_subject.chain = Some((head.length, head.digest));
        }
        self.health_subject.grant = self.kernel.current_grant().bits();
        self.last_health = self.health.evaluate(&self.health_subject);
        self.last_refresh = Instant::now();
        // Resolve current cluster route preview.
        self.cluster_test_node = self.ring.route_session(&self.cluster_test_key).map(|n| n.0);
    }

    /// Run one demo turn through the engine — used by the Dashboard's
    /// "press `t` to run a turn" affordance.
    pub async fn run_demo_turn(&mut self) {
        let obs = Observation::new(
            ObservationSource::User {
                channel: "gauss-tui".into(),
            },
            self.default_taint,
            serde_json::json!({"body": "demo from TUI"}),
        );
        let input = TurnInput {
            id: TurnId::new(self.next_turn),
            obs,
        };
        self.next_turn = self.next_turn.saturating_add(1);
        match self.engine.run_turn(input).await {
            Ok(summary) => {
                self.log(
                    "INFO",
                    format!(
                        "turn {} committed; chain length = {}",
                        summary.id.as_u128(),
                        summary.chain_head.length
                    ),
                );
                self.turn_history
                    .push_front(TurnHistoryEntry::from_summary(&summary, self.default_taint));
                if self.turn_history.len() > 256 {
                    self.turn_history.pop_back();
                }
                self.flash = Some((format!("turn {} committed", summary.id.as_u128()), "ok"));
            }
            Err(e) => {
                self.log("ERROR", format!("turn failed: {e}"));
                self.flash = Some((format!("turn failed: {e}"), "err"));
            }
        }
        self.refresh().await;
    }

    /// Pop the highlighted pending approval and send `decision` back.
    pub async fn decide_pending(&mut self, decision: ApprovalDecision) {
        let popped = {
            let mut q = self.pending.lock().await;
            if q.is_empty() {
                None
            } else {
                let idx = self.pending_cursor.min(q.len() - 1);
                q.remove(idx)
            }
        };
        if let Some(req) = popped {
            match self.decision_tx.send(decision.clone()).await {
                Ok(()) => self.log(
                    "INFO",
                    format!(
                        "approval decided for turn {} ({})",
                        req.request.turn_id.as_u128(),
                        match &decision {
                            ApprovalDecision::Approved { approver } => {
                                format!("approved by {approver}")
                            }
                            ApprovalDecision::Denied { approver, .. } => {
                                format!("denied by {approver}")
                            }
                            ApprovalDecision::Timeout => "timeout".to_string(),
                            _ => "other".to_string(),
                        }
                    ),
                ),
                Err(e) => self.log("ERROR", format!("decision dispatch failed: {e}")),
            }
            if self.pending_cursor > 0 {
                self.pending_cursor = self.pending_cursor.saturating_sub(1);
            }
        }
    }

    /// Cluster admin: route a session id through the ring.
    pub fn route_cluster(&mut self) {
        self.cluster_test_node = self.ring.route_session(&self.cluster_test_key).map(|n| n.0);
        self.log(
            "INFO",
            format!(
                "routed `{}` → {:?}",
                self.cluster_test_key, self.cluster_test_node
            ),
        );
    }

    /// Cluster admin: add a node.
    pub fn add_cluster_node(&mut self, name: &str) {
        self.ring.add_node(ClusterNodeId::new(name));
        self.log("INFO", format!("added cluster node {name}"));
    }

    /// Cluster admin: remove a node.
    pub fn remove_cluster_node(&mut self, name: &str) {
        self.ring.remove_node(&ClusterNodeId::new(name));
        self.log("INFO", format!("removed cluster node {name}"));
    }

    /// Append a log line.
    pub fn log(&mut self, level: &'static str, message: impl Into<String>) {
        self.logs.push_back(LogLine {
            level,
            message: message.into(),
            ts_ms: now_ms(),
        });
        if self.logs.len() > 512 {
            self.logs.pop_front();
        }
    }

    /// Toggle taint band for new demo turns.
    pub fn cycle_taint(&mut self) {
        self.default_taint = match self.default_taint {
            TaintLabel::Trusted => TaintLabel::User,
            TaintLabel::User => TaintLabel::Web,
            TaintLabel::Web => TaintLabel::Adversarial,
            TaintLabel::Adversarial => TaintLabel::Trusted,
        };
    }

    /// Generate synthetic approval-pending entries for demo / training.
    pub async fn seed_demo_approval(&mut self) {
        use gauss_core::{ToolAction, ToolId};
        use gauss_sag::Risk;
        let req = ApprovalRequest::new(
            TurnId::new(self.next_turn.wrapping_add(1000)),
            ToolAction::new(
                ToolId("send_email".into()),
                serde_json::json!({"to":"ops@example.com","body":"deploy"}),
                CapToken::NETWORK_POST,
                false,
            ),
            Risk::RequireApproval,
            "non_reversible_high_impact",
        );
        self.pending.lock().await.push_back(PendingApproval {
            request: req,
            received_at: Instant::now(),
        });
        self.log("INFO", "seeded demo approval request");
    }
}

#[allow(clippy::cast_possible_truncation)]
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}
