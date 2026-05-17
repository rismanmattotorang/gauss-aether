//! [`CsvParseTool`] — RFC 4180 CSV → JSON array of objects.
//!
//! Pure-compute, no caps. Handles quoted fields, escaped quotes, and
//! CRLF / LF line endings. The first row is treated as the header; each
//! subsequent row becomes a JSON object keyed by that header.
//!
//! A minimal RFC 4180 parser is inlined here to avoid a new workspace
//! dependency. The state machine is small enough to audit at a glance.

use async_trait::async_trait;
use gauss_core::{GaussError, GaussResult, ToolId};
use gauss_traits::{ToolManifest, ToolTrait};
use gaussclaw_skill::SkillManifest;

const MANIFEST_TOML: &str = r#"
name        = "csv_parse"
description = "Parse an RFC 4180 CSV string into an array of objects keyed by the header row."
usage       = "Use to read a small CSV payload into structured JSON. Args: {input: string, delimiter?: \",\"}."
caps        = []
taint       = "trusted"
reversible  = true
persistent  = false

[guards]
no_instruction_substrings = true
max_string_len            = 262144

[schema]
type = "object"
"#;

/// Hard upper bound — refuse pathological payloads.
const MAX_FIELDS: usize = 100_000;

/// CSV parser tool.
pub struct CsvParseTool {
    manifest: ToolManifest,
}

impl CsvParseTool {
    /// Build a new CSV parser.
    ///
    /// # Panics
    /// Build-time only if the embedded manifest TOML fails to parse.
    #[must_use]
    pub fn new() -> Self {
        let skill = SkillManifest::from_toml(MANIFEST_TOML).expect("embedded skill toml");
        let manifest = skill
            .compile(ToolId("csv_parse".into()))
            .expect("embedded skill compiles");
        Self { manifest }
    }
}

impl Default for CsvParseTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolTrait for CsvParseTool {
    fn manifest(&self) -> &ToolManifest {
        &self.manifest
    }

    async fn invoke_raw(&self, args: serde_json::Value) -> GaussResult<serde_json::Value> {
        let input = args
            .get("input")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GaussError::Internal("missing string field `input`".into()))?;
        let delimiter = args
            .get("delimiter")
            .and_then(|v| v.as_str())
            .and_then(|s| s.chars().next())
            .unwrap_or(',');

        let rows = parse_rfc4180(input, delimiter)
            .map_err(|e| GaussError::Internal(format!("csv parse: {e}")))?;

        let mut iter = rows.into_iter();
        let header = iter
            .next()
            .ok_or_else(|| GaussError::Internal("csv input is empty".into()))?;

        let mut records = Vec::with_capacity(iter.size_hint().0);
        for row in iter {
            if row.iter().all(String::is_empty) {
                continue; // tolerate blank trailing line
            }
            let mut obj = serde_json::Map::with_capacity(header.len());
            for (i, key) in header.iter().enumerate() {
                let value = row.get(i).cloned().unwrap_or_default();
                obj.insert(key.clone(), serde_json::Value::String(value));
            }
            records.push(serde_json::Value::Object(obj));
        }

        Ok(serde_json::json!({
            "header": header,
            "records": records,
            "count": records.len(),
        }))
    }
}

/// Parse an RFC 4180 CSV string into rows of fields. State machine:
/// `Start`, `Unquoted`, `Quoted`, `QuotedEscape`.
fn parse_rfc4180(input: &str, delimiter: char) -> Result<Vec<Vec<String>>, &'static str> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut state = State::Start;
    let mut total_fields = 0usize;

    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            State::Start => match c {
                '"' => state = State::Quoted,
                d if d == delimiter => {
                    row.push(std::mem::take(&mut field));
                    total_fields = total_fields.saturating_add(1);
                    if total_fields > MAX_FIELDS {
                        return Err("too many fields");
                    }
                }
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                '\r' => {
                    // consume optional \n
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                }
                _ => {
                    field.push(c);
                    state = State::Unquoted;
                }
            },
            State::Unquoted => match c {
                d if d == delimiter => {
                    row.push(std::mem::take(&mut field));
                    total_fields = total_fields.saturating_add(1);
                    if total_fields > MAX_FIELDS {
                        return Err("too many fields");
                    }
                    state = State::Start;
                }
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                    state = State::Start;
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                    state = State::Start;
                }
                _ => field.push(c),
            },
            State::Quoted => match c {
                '"' => state = State::QuotedEscape,
                _ => field.push(c),
            },
            State::QuotedEscape => match c {
                '"' => {
                    field.push('"');
                    state = State::Quoted;
                }
                d if d == delimiter => {
                    row.push(std::mem::take(&mut field));
                    state = State::Start;
                }
                '\n' => {
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                    state = State::Start;
                }
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    row.push(std::mem::take(&mut field));
                    rows.push(std::mem::take(&mut row));
                    state = State::Start;
                }
                _ => return Err("malformed quote in csv"),
            },
        }
    }
    // Flush trailing field / row.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

#[derive(Debug, Clone, Copy)]
enum State {
    Start,
    Unquoted,
    Quoted,
    QuotedEscape,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_simple_csv() {
        let t = CsvParseTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": "a,b,c\n1,2,3\n4,5,6\n",
            }))
            .await
            .unwrap();
        assert_eq!(out["count"], 2);
        assert_eq!(out["records"][0]["a"], "1");
        assert_eq!(out["records"][1]["c"], "6");
    }

    #[tokio::test]
    async fn handles_quoted_fields_with_commas() {
        let t = CsvParseTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": "name,note\n\"Smith, J\",\"hello, world\"\n",
            }))
            .await
            .unwrap();
        assert_eq!(out["records"][0]["name"], "Smith, J");
        assert_eq!(out["records"][0]["note"], "hello, world");
    }

    #[tokio::test]
    async fn handles_escaped_quotes() {
        let t = CsvParseTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": "a\n\"he said \"\"hi\"\"\"\n",
            }))
            .await
            .unwrap();
        assert_eq!(out["records"][0]["a"], "he said \"hi\"");
    }

    #[tokio::test]
    async fn handles_crlf_line_endings() {
        let t = CsvParseTool::new();
        let out = t
            .invoke_raw(serde_json::json!({ "input": "a,b\r\n1,2\r\n" }))
            .await
            .unwrap();
        assert_eq!(out["count"], 1);
        assert_eq!(out["records"][0]["b"], "2");
    }

    #[tokio::test]
    async fn rejects_empty_input() {
        let t = CsvParseTool::new();
        let err = t.invoke_raw(serde_json::json!({ "input": "" })).await.unwrap_err();
        assert!(matches!(err, GaussError::Internal(_)));
    }

    #[tokio::test]
    async fn custom_delimiter() {
        let t = CsvParseTool::new();
        let out = t
            .invoke_raw(serde_json::json!({
                "input": "a;b\n1;2\n",
                "delimiter": ";",
            }))
            .await
            .unwrap();
        assert_eq!(out["records"][0]["a"], "1");
        assert_eq!(out["records"][0]["b"], "2");
    }
}
