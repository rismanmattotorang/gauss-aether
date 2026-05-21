# OpenHarness Parity Matrix

*Last reviewed: 2026-05-21. Current state: Sprint 13 closed.
Known-gaps are owned by Sprint 14 → 15 (`/ROADMAP.md`) and Phase 6
(`/gaussclaw/ROADMAP.md`).*

This document maps every [OpenHarness](https://github.com/HKUDS/OpenHarness)
subsystem to its GaussClaw / Gauss-Aether equivalent. For each row:

- **Module(s)** — where the implementation lives.
- **Opt-in** — the builder call or attachment point that activates the
  feature; empty when the feature is on by default.
- **Test demonstrating it** — one canonical test the reader can run to
  see the surface working.
- **Status** — `ready` (production-quality), `built-in only` (works
  against built-in fixtures; real-world interop unverified), or
  `stub` (placeholder pending wiring).

The matrix is authoritative: if a row says `ready` but the cited test
doesn't pass on `main`, the row is wrong. Update it.

---

## 1 — Engine (streaming tool-call loop)

| | |
|---|---|
| **Module** | [`gaussclaw_agent::AgentLoop`](../gaussclaw/crates/gaussclaw-agent/src/agent_loop.rs) |
| **Opt-in** | `AgentLoop::new(turn_policy)` |
| **Demo test** | `agent_loop::tests::inline_tool_call_drives_one_dispatch_then_stops` |
| **Status** | `ready` |

Iterates provider → parse tool calls → dispatch → repeat. Emits
`LoopEvent` per boundary; honours cancellation via `LoopSink::should_cancel`.

---

## 2 — Tools

| | |
|---|---|
| **Module** | [`gaussclaw_tools`](../gaussclaw/crates/gaussclaw-tools/src/) |
| **Opt-in** | `TurnPolicy::with_tools(ToolRegistry)` |
| **Demo test** | `gaussclaw_tools::tests::default_registry_has_nineteen_tools` |
| **Status** | `ready` |

19-tool default catalogue + `memory_md_read` / `memory_md_write` for
`MEMORY.md`. Every tool runs through the HWCA schema gate (Axiom A7).

---

## 3 — Skills (on-demand markdown knowledge)

| | |
|---|---|
| **Module** | [`gaussclaw_skill::MarkdownSkill`](../gaussclaw/crates/gaussclaw-skill/src/markdown_skill.rs) |
| **Injection** | [`gaussclaw_agent::MarkdownSkillEnricher`](../gaussclaw/crates/gaussclaw-agent/src/enrich_impls.rs) → `AgentLoop::with_enricher(...)` |
| **Demo test** | `enrich_impls::tests::markdown_skill_enricher_renders_each_discovered_skill` |
| **Status** | `ready` |

Anthropic-compatible `SKILL.md` format with YAML frontmatter,
`from_dir` / `discover_in` loaders, frontmatter caps bridge to
`CapToken`, symlink leaf refused. Enricher renders each skill as a
`## <name>` section.

---

## 4 — Plugins

| | |
|---|---|
| **Module** | [`gaussclaw_plugins`](../gaussclaw/crates/gaussclaw-plugins/src/lib.rs) + [`hook_factory`](../gaussclaw/crates/gaussclaw-plugins/src/hook_factory.rs) |
| **Opt-in** | `PluginRegistry::register(...)` + `register_hooks(&bus, &factory)` |
| **Demo test** | `hook_factory::tests::register_hooks_registers_pre_and_post` |
| **Status** | `ready` |

5 plugin kinds, cap-declared manifests, BLAKE3 provenance digest,
declared-hook resolution via `HookFactory` (built-in
`DefaultHookFactory` ships `dry-run-preview`, `shell-guard`,
`audit-log`).

---

## 5 — Permissions / Capability lattice

| | |
|---|---|
| **Module** | [`gauss_core::CapToken`](../gauss-aether/crates/gauss-core/src/cap.rs) + [`gauss_kernel::admit`](../gauss-aether/crates/gauss-kernel/src/admit.rs) |
| **Opt-in** | always on — kernel gates every action |
| **Demo test** | `gauss_kernel::admit::axiom_a2_capability_monotonicity` |
| **Status** | `ready` |

Axiom A2 — caps shrink only; growth is a compile-time refusal.
Outperforms OpenHarness's path-rule list because the lattice is data,
not strings.

---

## 6 — Hooks (PreToolUse / PostToolUse lifecycle)

| | |
|---|---|
| **Module** | [`gauss_hooks`](../gauss-aether/crates/gauss-hooks/src/lib.rs) |
| **Opt-in** | `AgentLoop::with_hooks(HookBus)` |
| **Audit integration** | `AgentLoop::with_audit(AuditTrace)` → `AuditEntry::{HookDeny, HookWarn}` |
| **Demo test** | `agent_loop::tests::hook_deny_appends_to_audit_chain` |
| **Status** | `ready` |

Capability-gated: hooks can `Warn` or `Deny` but never widen caps.
Args hashed (BLAKE3), never logged raw, so secrets cannot leak via
the receipt chain.

---

## 7 — Slash commands

| | |
|---|---|
| **Module** | [`gaussclaw_cli::slash`](../gaussclaw/crates/gaussclaw-cli/src/slash.rs) |
| **TUI consumption** | [`gaussclaw_tui::App.slash_registry`](../gaussclaw/crates/gaussclaw-tui/src/lib.rs) — `/commands`, "did you mean?" |
| **Demo test** | `gaussclaw_tui::tests::slash_commands_lists_registry_entries` |
| **Status** | `ready` (discoverability) / `stub` (plugin-registered dispatch) |

Registry + `parse_slash` + help renderer. TUI consults the registry
for the `/commands` listing and Levenshtein-distance-2 suggestions;
typed dispatch of plugin-registered commands still routes via the
hand-written match (placeholder response).

---

## 8 — MCP (Model Context Protocol)

| | |
|---|---|
| **Module** | [`gaussclaw_tools::mcp`](../gaussclaw/crates/gaussclaw-tools/src/mcp.rs) + [`mcp_http`](../gaussclaw/crates/gaussclaw-tools/src/mcp_http.rs) |
| **Opt-in** | `McpBridge::new(client).build()` → tools join `ToolRegistry` |
| **Demo test** | `mcp_http::tests::bridge_dispatches_through_http_client` |
| **Status** | `built-in only` — works against `ScriptedHttp` and `MockMcpClient`; no real MCP server interop test in tree |

JSON-RPC 2.0 client speaking `tools/list` + `tools/call` over the
existing `HttpClient` trait. Every MCP-bridged tool runs through the
schema gate with `SchemaGuards::strict` (IPI defence).

---

## 9 — Memory (cross-session)

| | |
|---|---|
| **Format** | [`gaussclaw_skill::MemoryFile`](../gaussclaw/crates/gaussclaw-skill/src/memory_md.rs) |
| **Enricher** | [`gaussclaw_agent::MemoryFileEnricher`](../gaussclaw/crates/gaussclaw-agent/src/enrich_impls.rs) |
| **Tools** | [`gaussclaw_tools::memory_md`](../gaussclaw/crates/gaussclaw-tools/src/memory_md.rs) — `memory_md_read`, `memory_md_write` |
| **Demo test** | `memory_md::tests::write_then_read_round_trip` |
| **Status** | `ready` (single process) / `built-in only` (multi-process safety) |

`MEMORY.md` sectioned parse, atomic writes (write-temp-then-rename),
256 KiB byte cap with oldest-first eviction. The enricher injects the
rendered body as a leading system message; the tools let the agent
curate its own memory. Single-process serialisation via a Mutex; two
GaussClaw processes sharing a `MEMORY.md` would race on the byte cap
boundary.

---

## 10 — Coordinator (multi-agent)

| | |
|---|---|
| **Today** | [`gaussclaw_tools::subagent`](../gaussclaw/crates/gaussclaw-tools/src/subagent.rs) — `DelegateTool`, `MixtureOfAgentsTool` |
| **Demo test** | `subagent::tests::mixture_runs_n_agents_and_returns_each_head` |
| **Status** | `stub` (one-shot subagent calls only — no team registry, no persistent identities, no background lifecycle) |

OpenHarness's "Swarm Coordination" remains a future sprint.

---

## OpenHarness-inspired extras not in OpenHarness

These exist *because* GaussClaw is a Gauss-Aether reference agent, but
are not in OpenHarness upstream:

- **Auto-Compaction with audit-chain witness** —
  `Compactor` trait + `WindowedCompactor` default + audit append.
  Demo: `agent_loop::tests::auto_compaction_appends_to_audit_chain`.
- **`PromptEnricher` composition + leading-system preservation by
  compactor** — enrichments survive context pressure by construction.
  Demo: `agent_loop::tests::enricher_prepends_leading_system_message`.
- **`CLAUDE.md` ancestor walk** — `ContextFileFinder` with depth cap +
  `.gaussclaw/STOP` short-circuit + symlink-leaf refusal.
  Demo: `context_file::tests::discover_returns_root_to_leaf_order`.
- **Dashboard `LoopEvent → wire` translation** —
  `gaussclaw_web::wire::loop_event_to_wire` + `WireLoopSink` +
  `chat_socket` end-to-end. Demo:
  `tests::chat_socket_path_streams_loop_events_via_wire`.
- **Status flag for production wiring** — `/api/status` reports
  `agent_attached` so smoke tests catch "looks complete in code, not
  reachable from the bin". Demo:
  `tests::status_endpoint_reports_agent_attached_when_wired`.

---

## Vendor codec wiring (Sprint 13)

| | |
|---|---|
| **Bin selection** | [`gaussclaw_providers::pick_provider`](../gaussclaw/crates/gaussclaw-providers/src/select.rs) reads `cfg.provider.name`; `gaussclaw-bin::run_web` calls it with the env-sourced API key |
| **Transport fallback** | [`UnconfiguredBackend`](../gaussclaw/crates/gaussclaw-providers/src/select.rs) — every send returns a clean `HttpError::Network("...not configured...")` until a real backend is plumbed |
| **End-to-end test** | `e2e_anthropic::anthropic_provider_drives_full_loop_one_turn` exercises `AnthropicProvider → MockHttpBackend → TurnPolicy → AgentLoop::run` with a canned Anthropic-shape response |
| **Status** | `ready` (config-driven selection + UnconfiguredBackend fallback + 6 e2e tests against AnthropicProvider + audit-chain integration verified) |

The bin now selects the vendor codec from config:
`anthropic` → `AnthropicProvider`, `openai` → `OpenAIProvider`,
empty / unknown → `EchoProvider` fallback. API key sourced from
`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`. Without a real HTTP backend
in the workspace, the chosen codec is wrapped around
`UnconfiguredBackend`; the dashboard surfaces that as an `error`
frame rather than silently returning a stub echo.

## Known gaps

These are honest known gaps as of sprint 13 (last reviewed
2026-05-21). Each is a candidate for the production-GA wave —
they're all owned by Sprint 14 → 15 in `/ROADMAP.md` and
Phase 6 §1, §3, §4, §9–§11 in `/gaussclaw/ROADMAP.md`.

1. **No real HTTP backend ships for providers.** ✅ for the *tools*
   side via `gaussclaw-http::ReqwestHttpClient` (Sprint 10 §1
   landed); ❌ for the *providers* side — `gaussclaw_providers`
   has its own internal `HttpBackend` trait and only ships
   `UnconfiguredBackend` today. Vendor codecs are reachable
   through `pick_provider` and demonstrated end-to-end against
   `MockHttpBackend`, but `gaussclaw serve` against
   `api.anthropic.com` still fails at the providers transport
   layer. **Owned by Sprint 14 §1**: new `gaussclaw-providers-http`
   crate sharing the existing `gaussclaw-http` TLS / client stack
   under the providers `HttpBackend` trait.
2. **No live-network smoke test.** End-to-end coverage is against
   `MockHttpBackend` only. A live-network test against the real
   Anthropic Messages API needs `ANTHROPIC_API_KEY` and a CI
   environment that allows outbound HTTPS; out of scope for
   `cargo test` today. **Owned by Sprint 14 §2**: `#[ignore]` +
   `live-network` cargo-feature-gated test, run only on the
   protected release runner.
3. **Plugin-registered slash commands surface in `/commands` but
   dispatch through a placeholder message.** Real wiring requires
   plumbing the plugin's command handler into the TUI's
   `dispatch_slash` match. **Owned by Sprint 14 §3.**
4. **MCP HTTP transport untested against a real MCP server.** Works
   against `ScriptedHttp` end-to-end (HTTP); the stdio transport
   ships via `StdioMcpClient` (Sprint 10 §9). No `cargo test`
   exercises an actual remote MCP server. **Owned by Sprint 14
   §4**: reference MCP echo server running in CI under
   docker-in-docker; live-network lane exercises round-trip.
5. **Multi-agent Coordinator stays one-shot.** OpenHarness's team
   registry + persistent agent identities + headless worker
   subprocesses are not built — only `DelegateTool` and
   `MixtureOfAgentsTool` (one-shot dispatches) ship today.
   **Owned by Sprint 15 §1–§3**: new `gaussclaw-coordinator`
   crate with `Team` / `AgentIdentity` / `TeamPolicy`,
   `gaussclaw worker` subprocess + UDS JSON-RPC, `TeamsPage` in
   the dashboard.

---

## How to extend this document

When you add a new feature inspired by OpenHarness:

1. Land the code + tests on `main` first.
2. Add a row above to the relevant subsystem (or a new subsystem
   section if it's new).
3. Cite the *exact* test that demonstrates the feature. If no test
   exists, the row's status is `stub`, not `ready`.
4. If you change an `opt-in` builder name, update both the row and
   the cited test name in the same commit.

This document is the contract between intent and implementation. It
should not lie.
