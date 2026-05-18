//! `gaussclaw-desktop` — Tauri 2 shell + IPC command surface.
//!
//! Phase 1 Task 5 of `GAUSSCLAW_ROADMAP.md`. Supersedes the upstream
//! Hermes Electron 39 desktop app on every measurable axis (see
//! `GAUSSCLAW_ROADMAP.md` § "Footprint targets" for the numbers).
//!
//! ## Architecture
//!
//! The desktop binary is **Tauri 2 + Rust** rendering through the OS
//! WebView (WebView2 / WKWebView / WebKitGTK — no Chromium bundled).
//! The frontend is the same `gaussclaw-web` React bundle, embedded
//! via `rust-embed` at build time. The IPC layer replaces Hermes's
//! HTTP-on-`127.0.0.1:8642` with typed `#[tauri::command]` calls over
//! OS-native IPC (Unix domain sockets / Windows named pipes).
//!
//! ## Crate layout
//!
//! - [`commands`] — the canonical IPC command surface. Pure async
//!   functions; tested without the Tauri runtime.
//! - [`state`] — shared application state. Holds the [`ServerState`]
//!   the Axum dashboard uses, so the IPC commands and the HTTP backend
//!   share one source of truth.
//! - [`build_info`] — version / profile metadata.
//! - When the `tauri-runtime` feature is on, [`run`] starts the actual
//!   desktop binary. Without the feature, the crate is still a usable
//!   library — every command is callable from Rust directly.
//!
//! ## Why a feature flag
//!
//! Tauri 2's system dependencies (`webkit2gtk-4.1` on Linux,
//! `WebView2` on Windows, `WKWebView` on macOS) are heavy and not
//! available in every CI environment. Gating the runtime behind
//! `tauri-runtime` lets the library half always compile and test;
//! shipping the desktop binary is a deliberate `cargo build
//! --features tauri-runtime` invocation.
//!
//! ## Superiorities over Hermes Desktop
//!
//! See `GAUSSCLAW_ROADMAP.md` § "Footprint targets". Headline:
//!
//! | metric | Hermes Desktop | GaussClaw target |
//! |---|---|---|
//! | installer | ~150 MB | ≤ 20 MB |
//! | RAM idle | ~250 MB | ≤ 80 MB |
//! | cold start | ~3 s | ≤ 500 ms |
//! | code-signed | no | yes (3 OSes) |
//! | IPC | HTTP on localhost | OS-native IPC |
//! | capability JSON | n/a | emitted from Skill Manifests |

#![allow(
    clippy::doc_markdown,
    clippy::missing_docs_in_private_items,
    clippy::unused_async,
    clippy::const_is_empty,
    clippy::too_long_first_doc_paragraph,
    clippy::too_many_lines,
    clippy::format_collect,
    clippy::items_after_statements,
    clippy::enum_glob_use,
    clippy::match_same_arms,
    clippy::format_in_format_args,
    clippy::wildcard_imports,
    clippy::format_push_string,
    clippy::arithmetic_side_effects,
    clippy::needless_borrows_for_generic_args
)]
#![allow(rustdoc::broken_intra_doc_links)]

pub mod commands;
pub mod state;
pub mod system;
pub mod updater;

#[cfg(feature = "tauri-runtime")]
pub mod runtime;

#[cfg(feature = "tauri-runtime")]
pub use runtime::run;

/// Build-time metadata exposed by [`commands::build_info`].
pub mod build_info {
    /// Crate version (`gaussclaw-desktop`).
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
    /// `"debug"` or `"release"`.
    pub const PROFILE: &str = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    /// Whether the binary linked the Tauri runtime.
    pub const TAURI_RUNTIME: bool = cfg!(feature = "tauri-runtime");
}

/// Canonical path of the Tauri configuration file relative to the
/// crate root. Used by both the runtime build and the config-parses
/// conformance test.
pub const TAURI_CONFIG_PATH: &str = "tauri.conf.json";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_is_populated() {
        assert!(!build_info::VERSION.is_empty());
        assert!(matches!(build_info::PROFILE, "debug" | "release"));
    }

    #[test]
    fn tauri_config_parses_as_json() {
        // Tauri 2 requires the config to be valid JSON with a known
        // top-level schema. We verify *structural* validity here; full
        // schema validation happens at `cargo tauri build` time.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(TAURI_CONFIG_PATH);
        let body = std::fs::read_to_string(&path).expect("tauri.conf.json missing");
        let v: serde_json::Value = serde_json::from_str(&body).expect("invalid JSON");

        // Required Tauri 2 top-level keys.
        for key in ["productName", "version", "identifier", "build", "app"] {
            assert!(v.get(key).is_some(), "missing required key: {key}");
        }
        // The identifier must be reverse-DNS shaped.
        let id = v["identifier"].as_str().unwrap();
        assert!(id.contains('.'), "identifier must be reverse-DNS: got {id}");
    }

    #[test]
    fn capabilities_default_parses_as_json() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("capabilities")
            .join("default.json");
        let body = std::fs::read_to_string(&path).expect("capabilities/default.json missing");
        let v: serde_json::Value = serde_json::from_str(&body).expect("invalid JSON");
        // Tauri 2 capability JSON has identifier + windows + permissions.
        assert!(v.get("identifier").is_some());
        assert!(v.get("windows").is_some());
        assert!(v.get("permissions").is_some());
    }
}
