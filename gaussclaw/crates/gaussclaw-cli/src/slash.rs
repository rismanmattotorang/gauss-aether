//! Data-driven slash-command registry.
//!
//! OpenHarness (HKUDS/OpenHarness) ships a slash-command surface — the
//! agent recognises `/help`, `/clear`, `/compact`, `/model`, etc., and
//! the catalogue is loaded as data so plugins can extend it. GaussClaw
//! today hard-codes the same names in `gaussclaw-tui::dispatch_slash`;
//! this module exposes the catalogue as data so other surfaces (web
//! dashboard, channel adapters, headless `chat`) can share it.
//!
//! ## Design goals
//!
//! 1. **Data first.** A [`SlashCommand`] is a small record (name,
//!    aliases, description, kind, requires_agent). The registry is a
//!    `BTreeMap` keyed by the canonical name. Listing is alphabetical
//!    so help output is stable.
//! 2. **No coupling to the TUI.** The registry doesn't execute; it
//!    only resolves a typed input into a `SlashCommand`. Each surface
//!    runs its own dispatch on top.
//! 3. **Compatible with the existing TUI match.** The default
//!    catalogue mirrors the existing hard-coded `/help`, `/quit`,
//!    `/clear`, `/info`, `/version`, `/copy`, `/history`, `/model`,
//!    `/receipt`, `/taint`, `/caps`, `/sandbox`, plus the
//!    "awaiting wiring" placeholders. Existing surfaces can swap to
//!    the registry without users noticing.
//! 4. **Plugin-friendly.** `register` is mutable and idempotent —
//!    plugins register their own commands at startup. Conflicts on
//!    canonical name are rejected; alias conflicts soft-shadow with
//!    a recorded note.
//!
//! ## Quick start
//!
//! ```
//! use gaussclaw_cli::slash::{SlashRegistry, parse_slash};
//! let reg = SlashRegistry::with_defaults();
//! let parsed = parse_slash("/help me please");
//! let head = parsed.as_ref().map(|p| p.head.as_str()).unwrap_or("");
//! let cmd = reg.resolve(head).unwrap();
//! assert_eq!(cmd.name, "help");
//! ```

#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

use std::collections::BTreeMap;

/// What kind of side effect this slash command typically has.
/// Surfaces use this to colour the help output and to decide whether
/// a command can run without an active agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SlashKind {
    /// Returns information / answers a question without changing state.
    Info,
    /// Mutates local UI state (history, clipboard, preferences).
    Local,
    /// Sends a request through the agent loop / provider plane.
    Agent,
    /// Calls the runtime (kernel, audit chain, sandbox, …).
    Runtime,
}

/// One registered slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SlashCommand {
    /// Canonical name (no leading `/`).
    pub name: &'static str,
    /// Optional aliases the registry also resolves.
    pub aliases: &'static [&'static str],
    /// One-line description for `/help` output.
    pub description: &'static str,
    /// Side-effect taxonomy.
    pub kind: SlashKind,
    /// `true` if the command requires the agent loop to be running.
    pub requires_agent: bool,
}

impl SlashCommand {
    /// Construct a static-data command.
    #[must_use]
    pub const fn new(
        name: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        kind: SlashKind,
        requires_agent: bool,
    ) -> Self {
        Self {
            name,
            aliases,
            description,
            kind,
            requires_agent,
        }
    }
}

/// The default catalogue. Mirrors the existing TUI hard-coded match so
/// surfaces can swap to the registry without behaviour drift.
pub const DEFAULTS: &[SlashCommand] = &[
    SlashCommand::new(
        "help",
        &["?"],
        "Show the slash-command tour.",
        SlashKind::Info,
        false,
    ),
    SlashCommand::new(
        "quit",
        &["exit"],
        "Exit the session.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "clear",
        &["new"],
        "Clear the visible history.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "version",
        &[],
        "Print GaussClaw version.",
        SlashKind::Info,
        false,
    ),
    SlashCommand::new(
        "info",
        &["status"],
        "Show session/model/turn/chain status.",
        SlashKind::Info,
        false,
    ),
    SlashCommand::new(
        "history",
        &[],
        "Show input-history persistence info.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "copy",
        &[],
        "Copy the last assistant reply via OSC 52.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "model",
        &[],
        "Show or set the active model.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "receipt",
        &[],
        "Print the receipt-chain head.",
        SlashKind::Runtime,
        false,
    ),
    SlashCommand::new(
        "taint",
        &[],
        "Print the current taint floor.",
        SlashKind::Runtime,
        false,
    ),
    SlashCommand::new(
        "caps",
        &[],
        "Show the number of granted capabilities.",
        SlashKind::Runtime,
        false,
    ),
    SlashCommand::new(
        "sandbox",
        &[],
        "List composite sandbox layers.",
        SlashKind::Runtime,
        false,
    ),
    // Agent-needing commands — placeholder bodies until full wiring.
    SlashCommand::new(
        "tools",
        &[],
        "List registered tools.",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "config",
        &[],
        "Inspect runtime configuration.",
        SlashKind::Runtime,
        true,
    ),
    SlashCommand::new("logs", &[], "Tail recent logs.", SlashKind::Runtime, true),
    SlashCommand::new(
        "queue",
        &[],
        "Show pending background work.",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "undo",
        &[],
        "Undo the last turn.",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "retry",
        &[],
        "Retry the last failed turn.",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "paste",
        &[],
        "Paste from the system clipboard.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "compact",
        &[],
        "Compact conversation history (OpenHarness Auto-Compaction).",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "resume",
        &[],
        "Resume a stored session.",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "sessions",
        &[],
        "List stored sessions.",
        SlashKind::Agent,
        true,
    ),
    SlashCommand::new(
        "details",
        &[],
        "Toggle verbose detail rendering.",
        SlashKind::Local,
        false,
    ),
    SlashCommand::new(
        "statusbar",
        &[],
        "Toggle the status bar visibility.",
        SlashKind::Local,
        false,
    ),
];

/// Result of parsing one slash-command line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParsedSlash {
    /// The command head (no leading `/`, lowercase).
    pub head: String,
    /// Remainder text after the first whitespace (trimmed).
    pub rest: String,
}

/// Parse one slash-command input line. Returns `None` if the input
/// doesn't start with `/` or the head is empty.
#[must_use]
pub fn parse_slash(input: &str) -> Option<ParsedSlash> {
    let raw = input.strip_prefix('/').unwrap_or(input.trim_start());
    if raw == input {
        // Input did not start with `/`; treat as a non-slash line.
        return None;
    }
    let (head_raw, rest_raw) = raw
        .split_once(char::is_whitespace)
        .map_or((raw, ""), |(h, r)| (h, r));
    let head = head_raw.trim().to_ascii_lowercase();
    if head.is_empty() {
        return None;
    }
    Some(ParsedSlash {
        head,
        rest: rest_raw.trim().to_owned(),
    })
}

/// In-memory slash-command registry. Plugins register additions at
/// startup; surfaces consult `resolve` per keystroke / per submit.
#[derive(Debug, Clone, Default)]
pub struct SlashRegistry {
    by_canonical: BTreeMap<String, SlashCommand>,
    aliases: BTreeMap<String, String>,
}

impl SlashRegistry {
    /// Build an empty registry. Tests and offline tooling start here;
    /// production prefers [`Self::with_defaults`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry pre-populated with [`DEFAULTS`].
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        for cmd in DEFAULTS {
            reg.register(*cmd).expect("default catalogue self-consistent");
        }
        reg
    }

    /// Register one command.
    ///
    /// # Errors
    /// Returns `Err` if a command with the same canonical name is
    /// already registered. Alias collisions are accepted but the new
    /// command shadows the prior alias mapping (recorded silently).
    pub fn register(&mut self, cmd: SlashCommand) -> Result<(), String> {
        let canonical = cmd.name.to_ascii_lowercase();
        if self.by_canonical.contains_key(&canonical) {
            return Err(format!("duplicate slash command: /{canonical}"));
        }
        for alias in cmd.aliases {
            self.aliases
                .insert(alias.to_ascii_lowercase(), canonical.clone());
        }
        self.by_canonical.insert(canonical, cmd);
        Ok(())
    }

    /// Resolve `head` (no leading `/`) to a command. Case-insensitive.
    /// Returns `None` if the head is unknown.
    #[must_use]
    pub fn resolve(&self, head: &str) -> Option<&SlashCommand> {
        let key = head.to_ascii_lowercase();
        if let Some(cmd) = self.by_canonical.get(&key) {
            return Some(cmd);
        }
        let canonical = self.aliases.get(&key)?;
        self.by_canonical.get(canonical)
    }

    /// Iterate every command in canonical name order. Suitable for
    /// `/help` rendering — the order is stable across releases.
    pub fn iter(&self) -> impl Iterator<Item = &SlashCommand> {
        self.by_canonical.values()
    }

    /// Number of registered commands (aliases not counted).
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_canonical.len()
    }

    /// `true` if empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_canonical.is_empty()
    }

    /// Format a sorted help table. The default TUI/web surfaces can
    /// emit this as the body of `/help`.
    #[must_use]
    pub fn help_text(&self) -> String {
        let mut out = String::with_capacity(self.by_canonical.len() * 48);
        out.push_str("Available slash commands:\n");
        for cmd in self.iter() {
            let alias_note = if cmd.aliases.is_empty() {
                String::new()
            } else {
                format!(" (aliases: {})", cmd.aliases.join(", "))
            };
            out.push_str(&format!(
                "  /{name:<10} {desc}{alias}\n",
                name = cmd.name,
                desc = cmd.description,
                alias = alias_note
            ));
        }
        out
    }
}

// ─── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_load_without_collision() {
        let reg = SlashRegistry::with_defaults();
        assert!(reg.len() >= 12);
        assert!(reg.resolve("help").is_some());
        assert!(reg.resolve("HELP").is_some());
        assert!(reg.resolve("?").is_some());
    }

    #[test]
    fn aliases_resolve_to_canonical() {
        let reg = SlashRegistry::with_defaults();
        let by_alias = reg.resolve("exit").unwrap();
        let by_canonical = reg.resolve("quit").unwrap();
        assert_eq!(by_alias.name, by_canonical.name);
    }

    #[test]
    fn unknown_command_returns_none() {
        let reg = SlashRegistry::with_defaults();
        assert!(reg.resolve("banana").is_none());
    }

    #[test]
    fn duplicate_registration_errors() {
        let mut reg = SlashRegistry::with_defaults();
        let dup = SlashCommand::new(
            "help",
            &[],
            "shadow attempt",
            SlashKind::Info,
            false,
        );
        let err = reg.register(dup).unwrap_err();
        assert!(err.contains("/help"));
    }

    #[test]
    fn plugin_command_register_and_resolve() {
        let mut reg = SlashRegistry::new();
        reg.register(SlashCommand::new(
            "deploy",
            &["ship"],
            "deploy the build",
            SlashKind::Agent,
            true,
        ))
        .unwrap();
        let cmd = reg.resolve("deploy").unwrap();
        assert!(matches!(cmd.kind, SlashKind::Agent));
        assert!(cmd.requires_agent);
        let by_alias = reg.resolve("ship").unwrap();
        assert_eq!(by_alias.name, "deploy");
    }

    #[test]
    fn iter_is_alphabetical() {
        let mut reg = SlashRegistry::new();
        for n in ["zeta", "alpha", "mu"] {
            reg.register(SlashCommand::new(n, &[], "x", SlashKind::Info, false))
                .unwrap();
        }
        let names: Vec<&str> = reg.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["alpha", "mu", "zeta"]);
    }

    #[test]
    fn parse_slash_extracts_head_and_rest() {
        let p = parse_slash("/model gpt-5").unwrap();
        assert_eq!(p.head, "model");
        assert_eq!(p.rest, "gpt-5");
    }

    #[test]
    fn parse_slash_lowercases_head() {
        let p = parse_slash("/HELP").unwrap();
        assert_eq!(p.head, "help");
        assert_eq!(p.rest, "");
    }

    #[test]
    fn parse_slash_returns_none_for_non_slash() {
        assert!(parse_slash("hi").is_none());
        assert!(parse_slash("/").is_none());
    }

    #[test]
    fn help_text_lists_every_command() {
        let reg = SlashRegistry::with_defaults();
        let help = reg.help_text();
        for cmd in reg.iter() {
            assert!(help.contains(&format!("/{}", cmd.name)));
        }
    }

    #[test]
    fn requires_agent_flag_set_correctly() {
        let reg = SlashRegistry::with_defaults();
        // /compact (OpenHarness Auto-Compaction) requires the agent.
        assert!(reg.resolve("compact").unwrap().requires_agent);
        // /help does not.
        assert!(!reg.resolve("help").unwrap().requires_agent);
    }
}
