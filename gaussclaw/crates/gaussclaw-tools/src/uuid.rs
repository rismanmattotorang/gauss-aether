//! [`UuidTool`] — generate UUIDv4 / UUIDv7 identifiers.
//!
//! Pure-compute, no caps. UUIDv4 is random; UUIDv7 is time-ordered and
//! sortable. Both implementations are inlined to avoid a new workspace
//! dependency — they're tiny and the RFC 4122 bit-layout is stable.

use async_trait::async_trait;
use gauss_core::{GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;
use rand_core::{OsRng, RngCore};
use time::OffsetDateTime;

const MANIFEST_TOML: &str = r#"
name        = "uuid"
description = "Generate one or more UUIDs. Args: {version?: 4|7, count?: u8}."
usage       = "Use for deterministic identifiers (v7 is time-ordered, v4 is random)."
caps        = []
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 4096

[schema]
type = "object"
"#;

/// Hard upper bound to keep one call cheap. The model can always re-invoke.
const MAX_COUNT: u64 = 64;

/// Pure-compute UUID generator.
pub struct UuidTool {
    manifest: ToolManifest,
}

impl UuidTool {
    /// Build a new UUID tool.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("uuid".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for UuidTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for UuidTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let version = args.get("version").and_then(|v| v.as_u64()).unwrap_or(4);
        let count = args
            .get("count")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .min(MAX_COUNT);
        let uuids: Vec<String> = (0..count)
            .map(|_| match version {
                7 => uuid_v7(),
                _ => uuid_v4(),
            })
            .collect();
        Ok(serde_json::json!({
            "version": version,
            "count": uuids.len(),
            "uuids": uuids,
        }))
    }
}

/// Emit one UUIDv4 (RFC 4122 § 4.4): 122 random bits plus version + variant.
fn uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant
    format_uuid(&bytes)
}

/// Emit one UUIDv7 (draft RFC, time-ordered): 48-bit unix-millisecond
/// prefix + 4-bit version + 12-bit random + 2-bit variant + 62-bit random.
fn uuid_v7() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let now = OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    // Truncate to 48 bits — UUIDv7 is defined to overflow in year 10889.
    let millis = (now as i64).max(0) as u64;
    bytes[0] = ((millis >> 40) & 0xFF) as u8;
    bytes[1] = ((millis >> 32) & 0xFF) as u8;
    bytes[2] = ((millis >> 24) & 0xFF) as u8;
    bytes[3] = ((millis >> 16) & 0xFF) as u8;
    bytes[4] = ((millis >> 8) & 0xFF) as u8;
    bytes[5] = (millis & 0xFF) as u8;
    bytes[6] = (bytes[6] & 0x0F) | 0x70; // version 7
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant
    format_uuid(&bytes)
}

fn format_uuid(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3],
        b[4], b[5],
        b[6], b[7],
        b[8], b[9],
        b[10], b[11], b[12], b[13], b[14], b[15],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_emits_one_v4() {
        let t = UuidTool::new();
        let out = t.invoke_raw(serde_json::json!({})).await.unwrap();
        assert_eq!(out["version"], 4);
        let uuids = out["uuids"].as_array().unwrap();
        assert_eq!(uuids.len(), 1);
        let s = uuids[0].as_str().unwrap();
        assert_eq!(s.len(), 36);
        // Version nibble: byte 6, low nibble.
        let v = s.chars().nth(14).unwrap();
        assert_eq!(v, '4', "version nibble should be 4: {s}");
    }

    #[tokio::test]
    async fn v7_is_time_ordered() {
        let t = UuidTool::new();
        let a = t
            .invoke_raw(serde_json::json!({ "version": 7 }))
            .await
            .unwrap();
        // Force a clock tick before the second sample.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let b = t
            .invoke_raw(serde_json::json!({ "version": 7 }))
            .await
            .unwrap();
        let aid = a["uuids"][0].as_str().unwrap();
        let bid = b["uuids"][0].as_str().unwrap();
        assert!(aid < bid, "v7 should be time-ordered: a={aid}, b={bid}");
    }

    #[tokio::test]
    async fn count_is_clamped_to_max() {
        let t = UuidTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "count": 9999 }))
            .await
            .unwrap();
        assert_eq!(out["count"], 64);
        assert_eq!(out["uuids"].as_array().unwrap().len(), 64);
    }
}
