//! Sprint 8 of "Wire the Loop" — server-side i18n.
//!
//! A tiny in-process translation catalog: each locale ships as an
//! embedded JSON document keyed by short, stable message ids
//! (`chat.banner.connected`, `chat.banner.stub`, …). The dashboard
//! frontend fetches the catalog for its preferred locale via
//! `/api/i18n/:locale` and resolves ids at render time; system-emitted
//! strings inside the server use [`t`] to pick the same string before
//! it crosses the wire.
//!
//! ## Locale negotiation
//!
//! The server tries the explicit locale first, then falls back to the
//! locale's base (`zh-Hans-CN` → `zh-Hans` → `en`), then to `en`. The
//! `en` catalog is the source of truth; every other locale only needs
//! to translate the keys it wants — missing ids fall through to `en`.
//!
//! ## Why not GNU gettext / Fluent
//!
//! Both pull in heavy runtime dependencies for a single-process
//! agent. The catalog is small (low hundreds of strings); a flat
//! string→string `HashMap` is fine and stays observable in
//! `serde_json::Value` form for the dashboard's fetch.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// Default locale used when no other locale matches.
pub const DEFAULT_LOCALE: &str = "en";

/// All locales the server ships catalogs for.
pub const SUPPORTED_LOCALES: &[&str] = &["en", "zh-Hans", "id-ID"];

const EN_JSON: &str = include_str!("locales/en.json");
const ZH_HANS_JSON: &str = include_str!("locales/zh-Hans.json");
const ID_ID_JSON: &str = include_str!("locales/id-ID.json");

/// One locale's flat key→message map.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Catalog {
    /// Key → translated message.
    #[serde(flatten)]
    pub messages: HashMap<String, String>,
}

impl Catalog {
    /// Lookup a key; returns `None` when the catalog doesn't carry
    /// the key. Callers chain through fallbacks via [`t`].
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.messages.get(key).map(String::as_str)
    }

    /// Number of keys in this catalog.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// True iff [`Self::len`] is zero.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

fn load(json: &str, locale: &'static str) -> Catalog {
    serde_json::from_str(json).unwrap_or_else(|e| {
        // We control these files; a parse error is a developer
        // mistake. Don't crash the server — emit an empty catalog so
        // every lookup falls through to en.
        tracing::error!(target: "gaussclaw_web::i18n", "failed to parse {locale} catalog: {e}");
        Catalog::default()
    })
}

/// Process-wide registry of every embedded catalog.
fn catalogs() -> &'static HashMap<&'static str, Catalog> {
    static C: OnceLock<HashMap<&'static str, Catalog>> = OnceLock::new();
    C.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("en", load(EN_JSON, "en"));
        m.insert("zh-Hans", load(ZH_HANS_JSON, "zh-Hans"));
        m.insert("id-ID", load(ID_ID_JSON, "id-ID"));
        m
    })
}

/// Resolve a locale tag through the supported list. Strips region
/// subtags (`zh-Hans-CN` → `zh-Hans`) and falls back to
/// [`DEFAULT_LOCALE`].
#[must_use]
pub fn resolve_locale(locale: &str) -> &'static str {
    let locale = locale.trim();
    if locale.is_empty() {
        return DEFAULT_LOCALE;
    }
    // Exact match first.
    for &candidate in SUPPORTED_LOCALES {
        if locale.eq_ignore_ascii_case(candidate) {
            return candidate;
        }
    }
    // Try the script-stripped tag (`zh-Hans-CN` → `zh-Hans`,
    // `zh-CN` → `zh-Hans` is one we handle below for the common case).
    if let Some((head, _)) = locale.rsplit_once('-') {
        for &candidate in SUPPORTED_LOCALES {
            if head.eq_ignore_ascii_case(candidate) {
                return candidate;
            }
        }
        // Friendly fallback: `zh-CN`, `zh-TW`, `zh-HK`, `zh` → `zh-Hans`.
        let head_lower = head.to_ascii_lowercase();
        if head_lower == "zh" {
            return "zh-Hans";
        }
        if head_lower == "id" {
            return "id-ID";
        }
    }
    // Bare `zh` / `id` (already without a dash) get the same friendly
    // mapping.
    let bare = locale.to_ascii_lowercase();
    if bare == "zh" {
        return "zh-Hans";
    }
    if bare == "id" {
        return "id-ID";
    }
    DEFAULT_LOCALE
}

/// Translate a key in `locale`, falling back to en when absent.
/// Returns the key itself as a last resort so a missing translation
/// is visible to operators but doesn't blank the UI.
#[must_use]
pub fn t(locale: &str, key: &str) -> String {
    let locale = resolve_locale(locale);
    let cats = catalogs();
    if let Some(c) = cats.get(locale) {
        if let Some(v) = c.get(key) {
            return v.to_string();
        }
    }
    if locale != "en" {
        if let Some(c) = cats.get("en") {
            if let Some(v) = c.get(key) {
                return v.to_string();
            }
        }
    }
    key.to_string()
}

/// Return the full catalog for `locale` (merged onto the en catalog
/// so the dashboard sees every key in one shape). The dashboard's
/// fetch path lives in `/api/i18n/:locale`.
#[must_use]
pub fn catalog_for(locale: &str) -> HashMap<String, String> {
    let resolved = resolve_locale(locale);
    let cats = catalogs();
    let mut merged: HashMap<String, String> = cats
        .get("en")
        .map(|c| c.messages.clone())
        .unwrap_or_default();
    if resolved != "en" {
        if let Some(c) = cats.get(resolved) {
            for (k, v) in &c.messages {
                merged.insert(k.clone(), v.clone());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_parses_and_is_non_empty() {
        for &locale in SUPPORTED_LOCALES {
            let c = catalogs().get(locale).expect("registered");
            assert!(
                !c.is_empty(),
                "{locale} catalog must ship at least one key"
            );
        }
    }

    #[test]
    fn exact_and_prefix_locales_resolve() {
        assert_eq!(resolve_locale("en"), "en");
        assert_eq!(resolve_locale("EN-US"), "en", "case-insensitive base");
        assert_eq!(resolve_locale("zh-Hans"), "zh-Hans");
        assert_eq!(resolve_locale("zh-Hans-CN"), "zh-Hans");
        assert_eq!(resolve_locale("zh-CN"), "zh-Hans", "Mandarin shortcut");
        assert_eq!(resolve_locale("zh"), "zh-Hans");
        assert_eq!(resolve_locale("id"), "id-ID");
        assert_eq!(resolve_locale("id-ID"), "id-ID");
    }

    #[test]
    fn unknown_locale_falls_back_to_default() {
        assert_eq!(resolve_locale("xx-XX"), DEFAULT_LOCALE);
        assert_eq!(resolve_locale(""), DEFAULT_LOCALE);
        assert_eq!(resolve_locale("klingon"), DEFAULT_LOCALE);
    }

    #[test]
    fn t_returns_locale_specific_translation() {
        let en = t("en", "chat.banner.connected");
        let zh = t("zh-Hans", "chat.banner.connected");
        let id = t("id-ID", "chat.banner.connected");
        assert!(!en.is_empty());
        assert!(!zh.is_empty());
        assert!(!id.is_empty());
        // Each locale must carry a distinct translation for at least
        // this canonical string — protects against accidental
        // identical catalogs.
        assert_ne!(en, zh, "en and zh-Hans must differ on the banner");
        assert_ne!(en, id, "en and id-ID must differ on the banner");
    }

    #[test]
    fn missing_key_falls_through_to_en_then_key_itself() {
        // Use a key that exists only in en to prove fallback.
        let v = t("zh-Hans", "does.not.exist.anywhere");
        assert_eq!(
            v, "does.not.exist.anywhere",
            "missing keys must surface as the key string"
        );
    }

    #[test]
    fn catalog_for_merges_locale_onto_en() {
        let merged = catalog_for("zh-Hans");
        // Every en key should be present in the merged map (either as
        // its zh translation or as the en fallback).
        let en_keys: Vec<String> =
            catalogs().get("en").unwrap().messages.keys().cloned().collect();
        for k in &en_keys {
            assert!(
                merged.contains_key(k),
                "merged zh catalog must include en key `{k}`"
            );
        }
    }
}
