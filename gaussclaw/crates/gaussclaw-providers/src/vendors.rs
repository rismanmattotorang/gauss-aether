//! Canonical list of the first-party vendor drivers this crate ships.
//!
//! A single source of truth for "which providers does GaussClaw
//! support" — surfaced by the dashboard's `/api/providers` endpoint and
//! usable by the CLI. This is a fact about the compiled binary (a
//! driver module exists for each), not a live model catalogue: model
//! ids are discovered per-vendor at request time.

/// Static descriptor for one supported vendor driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct VendorInfo {
    /// Stable lowercase id used in config (`provider.name`) and routing.
    pub id: &'static str,
    /// Human-readable display name for UIs.
    pub display: &'static str,
}

/// The twenty first-party vendor drivers, in a stable display order.
///
/// Matches the driver modules in this crate: the direct codecs
/// (`anthropic`, `openai`, `google`, `cohere`, `huggingface`,
/// `llama_cpp`, `ollama`, `replicate`) plus the OpenAI-compatible
/// factories (`groq`, `cerebras`, `fireworks`, `deepseek`, `mistral`,
/// `together`, `xai`, `perplexity`, `anyscale`, `octoai`, `vllm`,
/// `tgi`).
pub const SUPPORTED_VENDORS: &[VendorInfo] = &[
    VendorInfo { id: "anthropic", display: "Anthropic" },
    VendorInfo { id: "openai", display: "OpenAI" },
    VendorInfo { id: "google", display: "Google Gemini" },
    VendorInfo { id: "cohere", display: "Cohere" },
    VendorInfo { id: "mistral", display: "Mistral" },
    VendorInfo { id: "together", display: "Together" },
    VendorInfo { id: "groq", display: "Groq" },
    VendorInfo { id: "cerebras", display: "Cerebras" },
    VendorInfo { id: "fireworks", display: "Fireworks" },
    VendorInfo { id: "deepseek", display: "DeepSeek" },
    VendorInfo { id: "xai", display: "xAI" },
    VendorInfo { id: "perplexity", display: "Perplexity" },
    VendorInfo { id: "anyscale", display: "Anyscale" },
    VendorInfo { id: "octoai", display: "OctoAI" },
    VendorInfo { id: "huggingface", display: "HuggingFace" },
    VendorInfo { id: "replicate", display: "Replicate" },
    VendorInfo { id: "ollama", display: "Ollama" },
    VendorInfo { id: "llama_cpp", display: "llama.cpp" },
    VendorInfo { id: "vllm", display: "vLLM" },
    VendorInfo { id: "tgi", display: "TGI" },
];

/// The supported vendor drivers (see [`SUPPORTED_VENDORS`]).
#[must_use]
pub fn supported_vendors() -> &'static [VendorInfo] {
    SUPPORTED_VENDORS
}

/// True iff `id` (case-insensitively) names a supported vendor driver.
#[must_use]
pub fn is_supported_vendor(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    SUPPORTED_VENDORS.iter().any(|v| v.id == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ships_twenty_vendors() {
        assert_eq!(SUPPORTED_VENDORS.len(), 20);
    }

    #[test]
    fn ids_are_unique_and_lowercase() {
        let mut seen = std::collections::BTreeSet::new();
        for v in SUPPORTED_VENDORS {
            assert_eq!(v.id, v.id.to_ascii_lowercase(), "id {} not lowercase", v.id);
            assert!(seen.insert(v.id), "duplicate vendor id {}", v.id);
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert!(is_supported_vendor("anthropic"));
        assert!(is_supported_vendor("ANTHROPIC"));
        assert!(is_supported_vendor("OpenAI"));
        assert!(!is_supported_vendor("not-a-vendor"));
    }
}
