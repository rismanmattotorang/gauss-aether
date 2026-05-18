//! Schedule grammar — three accepted input forms.
//!
//! - **Duration** — `30s`, `15m`, `2h30m`, `1d12h`. Fires once after
//!   the duration elapses from the schedule's creation time.
//! - **Cron expression** — five-field POSIX cron
//!   `minute hour day-of-month month day-of-week`, with `*`, `*/N`,
//!   and bare integers. Reschedules on every fire.
//! - **ISO 8601 timestamp** — `2026-05-20T14:30:00Z`. Fires once at
//!   the absolute moment.
//!
//! The grammar is deliberately *narrow*: cron expressions support
//! the union of features users actually reach for (`*/15 * * * *`,
//! `0 9 * * 1-5`), not the full cron-compatibility lore. Hermes
//! accepts only cron expressions; GaussClaw adds durations and
//! absolute timestamps because they're what an agent prompt
//! actually produces.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

/// One parsed schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Schedule {
    /// Fire once `seconds` after schedule creation.
    Duration {
        /// Total delay, in seconds.
        seconds: i64,
    },
    /// Fire at every cron-expression match. Stored as five
    /// already-parsed fields so the firing-time computation never
    /// re-parses.
    Cron {
        /// Original cron string (for round-trip serialisation).
        expr: String,
        /// Allowed minute values `[0..=59]`.
        minute: CronField,
        /// Allowed hour values `[0..=23]`.
        hour: CronField,
        /// Allowed day-of-month values `[1..=31]`.
        dom: CronField,
        /// Allowed month values `[1..=12]`.
        month: CronField,
        /// Allowed day-of-week values `[0..=6]` (Sunday = 0).
        dow: CronField,
    },
    /// Fire once at the given UTC instant.
    At {
        /// UTC Unix timestamp (seconds).
        unix_seconds: i64,
    },
}

/// A parsed cron field. Either a full wildcard, a `*/N` step, or
/// a bounded set of allowed values (handles `1,3-5,7` etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CronField {
    /// `*` — matches every value in the field's range.
    Any,
    /// `*/N` — matches values where `v % N == 0`.
    Step(u8),
    /// Bounded explicit value set.
    Set(Vec<u8>),
}

impl CronField {
    /// True iff `value` matches this field.
    #[must_use]
    pub fn matches(&self, value: u8) -> bool {
        match self {
            Self::Any => true,
            Self::Step(n) => *n != 0 && value.checked_rem(*n).is_some_and(|r| r == 0),
            Self::Set(set) => set.contains(&value),
        }
    }
}

/// Errors from [`parse_schedule`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScheduleParseError {
    /// Input didn't match any accepted form.
    #[error("schedule did not match duration, cron, or ISO 8601 grammar: {0:?}")]
    Unrecognised(String),
    /// Duration grammar specifically rejected the input.
    #[error("invalid duration `{0}`: {1}")]
    InvalidDuration(String, &'static str),
    /// Cron grammar specifically rejected the input.
    #[error("invalid cron expression `{0}`: {1}")]
    InvalidCron(String, String),
    /// ISO 8601 grammar specifically rejected the input.
    #[error("invalid ISO 8601 timestamp `{0}`")]
    InvalidIso(String),
}

/// Parse a schedule string. Tries duration → cron → ISO in order.
///
/// # Errors
/// Returns [`ScheduleParseError`] naming the failing axis.
pub fn parse_schedule(input: &str) -> Result<Schedule, ScheduleParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ScheduleParseError::Unrecognised(String::new()));
    }
    // ISO 8601 timestamps are unambiguous: contain `T` and `:`.
    if trimmed.contains('T') && trimmed.contains(':') {
        return parse_iso(trimmed);
    }
    // Cron expressions: contain `*`, `,`, `-`, or `/`, OR have any
    // whitespace at all and don't look duration-shaped. Durations
    // like `2h 15m` are space-tolerant but contain no cron-only
    // metacharacters, so an input with both spaces and metacharacters
    // routes to cron (where field-count violations surface as
    // `InvalidCron`, not as an unrelated duration error).
    let has_cron_meta =
        trimmed.contains(['*', ',', '/']) || (trimmed.contains('-') && trimmed.contains(' '));
    if has_cron_meta || trimmed.split_whitespace().count() >= 3 {
        return parse_cron(trimmed);
    }
    // Otherwise try duration.
    parse_duration(trimmed)
}

// ─── duration ──────────────────────────────────────────────────────────────

fn parse_duration(input: &str) -> Result<Schedule, ScheduleParseError> {
    let mut total: i64 = 0;
    let mut buf = String::new();
    for c in input.chars() {
        if c.is_ascii_digit() {
            buf.push(c);
        } else if matches!(c, 's' | 'm' | 'h' | 'd') {
            if buf.is_empty() {
                return Err(ScheduleParseError::InvalidDuration(
                    input.into(),
                    "unit without preceding number",
                ));
            }
            let n: i64 = buf.parse().map_err(|_| {
                ScheduleParseError::InvalidDuration(input.into(), "numeric overflow")
            })?;
            buf.clear();
            let mult: i64 = match c {
                's' => 1,
                'm' => 60,
                'h' => 3_600,
                'd' => 86_400,
                _ => unreachable!(),
            };
            total = total.checked_add(n.saturating_mul(mult)).ok_or(
                ScheduleParseError::InvalidDuration(input.into(), "duration overflow"),
            )?;
        } else if c.is_whitespace() {
            // Allow `2h 15m` for ergonomics.
            continue;
        } else {
            return Err(ScheduleParseError::Unrecognised(input.into()));
        }
    }
    if !buf.is_empty() {
        return Err(ScheduleParseError::InvalidDuration(
            input.into(),
            "trailing digits without unit",
        ));
    }
    if total <= 0 {
        return Err(ScheduleParseError::InvalidDuration(
            input.into(),
            "duration must be positive",
        ));
    }
    Ok(Schedule::Duration { seconds: total })
}

// ─── cron ──────────────────────────────────────────────────────────────────

fn parse_cron(input: &str) -> Result<Schedule, ScheduleParseError> {
    let fields: Vec<&str> = input.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(ScheduleParseError::InvalidCron(
            input.into(),
            format!("expected 5 fields, got {}", fields.len()),
        ));
    }
    let minute = parse_cron_field(fields[0], 0, 59, "minute")
        .map_err(|m| ScheduleParseError::InvalidCron(input.into(), m))?;
    let hour = parse_cron_field(fields[1], 0, 23, "hour")
        .map_err(|m| ScheduleParseError::InvalidCron(input.into(), m))?;
    let dom = parse_cron_field(fields[2], 1, 31, "day-of-month")
        .map_err(|m| ScheduleParseError::InvalidCron(input.into(), m))?;
    let month = parse_cron_field(fields[3], 1, 12, "month")
        .map_err(|m| ScheduleParseError::InvalidCron(input.into(), m))?;
    let dow = parse_cron_field(fields[4], 0, 6, "day-of-week")
        .map_err(|m| ScheduleParseError::InvalidCron(input.into(), m))?;
    Ok(Schedule::Cron {
        expr: input.into(),
        minute,
        hour,
        dom,
        month,
        dow,
    })
}

fn parse_cron_field(s: &str, min: u8, max: u8, name: &str) -> Result<CronField, String> {
    if s == "*" {
        return Ok(CronField::Any);
    }
    if let Some(rest) = s.strip_prefix("*/") {
        let n: u8 = rest
            .parse()
            .map_err(|_| format!("{name} step `{s}` not numeric"))?;
        if n == 0 {
            return Err(format!("{name} step must be > 0"));
        }
        return Ok(CronField::Step(n));
    }
    // Otherwise: comma-separated list of bare ints or `a-b` ranges.
    let mut acc: Vec<u8> = Vec::new();
    for tok in s.split(',') {
        if let Some((a, b)) = tok.split_once('-') {
            let lo: u8 = a
                .parse()
                .map_err(|_| format!("{name} range lo `{a}` not numeric"))?;
            let hi: u8 = b
                .parse()
                .map_err(|_| format!("{name} range hi `{b}` not numeric"))?;
            if lo > hi {
                return Err(format!("{name} range `{tok}` reversed"));
            }
            if lo < min || hi > max {
                return Err(format!("{name} range `{tok}` out of bounds [{min},{max}]"));
            }
            acc.extend(lo..=hi);
        } else {
            let v: u8 = tok
                .parse()
                .map_err(|_| format!("{name} value `{tok}` not numeric"))?;
            if v < min || v > max {
                return Err(format!("{name} value `{tok}` out of bounds [{min},{max}]"));
            }
            acc.push(v);
        }
    }
    acc.sort_unstable();
    acc.dedup();
    Ok(CronField::Set(acc))
}

// ─── ISO 8601 ──────────────────────────────────────────────────────────────

fn parse_iso(input: &str) -> Result<Schedule, ScheduleParseError> {
    // RFC 3339 is the strict-profile subset of ISO 8601 that all
    // agents emit (`2026-05-20T14:30:00Z`). Drop sub-second precision
    // since the scheduler ticks at minute granularity anyway.
    let parsed = OffsetDateTime::parse(input, &time::format_description::well_known::Rfc3339)
        .map_err(|_| ScheduleParseError::InvalidIso(input.into()))?;
    Ok(Schedule::At {
        unix_seconds: parsed.unix_timestamp(),
    })
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duration { seconds } => write!(f, "in:{seconds}s"),
            Self::Cron { expr, .. } => write!(f, "cron:{expr}"),
            Self::At { unix_seconds } => write!(f, "at:{unix_seconds}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parses_simple_units() {
        assert_eq!(
            parse_schedule("30s").unwrap(),
            Schedule::Duration { seconds: 30 }
        );
        assert_eq!(
            parse_schedule("15m").unwrap(),
            Schedule::Duration { seconds: 900 }
        );
        assert_eq!(
            parse_schedule("2h").unwrap(),
            Schedule::Duration { seconds: 7_200 }
        );
        assert_eq!(
            parse_schedule("1d").unwrap(),
            Schedule::Duration { seconds: 86_400 }
        );
    }

    #[test]
    fn duration_combines_units() {
        assert_eq!(
            parse_schedule("2h15m").unwrap(),
            Schedule::Duration { seconds: 8_100 }
        );
        assert_eq!(
            parse_schedule("1d12h").unwrap(),
            Schedule::Duration {
                seconds: 86_400 + 43_200
            }
        );
    }

    #[test]
    fn duration_allows_whitespace() {
        assert_eq!(
            parse_schedule("2h 15m").unwrap(),
            Schedule::Duration { seconds: 8_100 }
        );
    }

    #[test]
    fn duration_rejects_zero_or_missing_unit() {
        assert!(matches!(
            parse_schedule("0m"),
            Err(ScheduleParseError::InvalidDuration(_, _))
        ));
        assert!(matches!(
            parse_schedule("30"),
            Err(ScheduleParseError::InvalidDuration(_, _))
        ));
    }

    #[test]
    fn cron_parses_five_fields() {
        let s = parse_schedule("*/15 * * * *").unwrap();
        match s {
            Schedule::Cron { minute, hour, .. } => {
                assert_eq!(minute, CronField::Step(15));
                assert_eq!(hour, CronField::Any);
            }
            _ => panic!("expected Cron"),
        }
    }

    #[test]
    fn cron_parses_ranges_and_lists() {
        let s = parse_schedule("0 9 * * 1-5").unwrap();
        match s {
            Schedule::Cron {
                minute, hour, dow, ..
            } => {
                assert_eq!(minute, CronField::Set(vec![0]));
                assert_eq!(hour, CronField::Set(vec![9]));
                assert_eq!(dow, CronField::Set(vec![1, 2, 3, 4, 5]));
            }
            _ => panic!("expected Cron"),
        }
    }

    #[test]
    fn cron_field_matches_correctly() {
        assert!(CronField::Any.matches(7));
        assert!(CronField::Step(15).matches(0));
        assert!(CronField::Step(15).matches(30));
        assert!(!CronField::Step(15).matches(13));
        assert!(CronField::Set(vec![1, 3, 5]).matches(3));
        assert!(!CronField::Set(vec![1, 3, 5]).matches(4));
    }

    #[test]
    fn cron_rejects_bad_field_counts() {
        assert!(matches!(
            parse_schedule("* * * *"),
            Err(ScheduleParseError::InvalidCron(_, _))
        ));
    }

    #[test]
    fn cron_rejects_out_of_range_values() {
        let err = parse_schedule("99 * * * *").unwrap_err();
        assert!(matches!(err, ScheduleParseError::InvalidCron(_, _)));
    }

    #[test]
    fn iso_parses_utc_timestamp() {
        let s = parse_schedule("2026-05-20T14:30:00Z").unwrap();
        match s {
            Schedule::At { unix_seconds } => {
                // 2026-05-20T14:30:00Z = 1779287400 (verified via
                // OffsetDateTime round-trip).
                assert_eq!(unix_seconds, 1_779_287_400);
            }
            _ => panic!("expected At"),
        }
    }

    #[test]
    fn iso_rejects_malformed_input() {
        assert!(matches!(
            parse_schedule("2026-05-20T99:99:99Z"),
            Err(ScheduleParseError::InvalidIso(_))
        ));
    }

    #[test]
    fn empty_input_rejected() {
        assert!(matches!(
            parse_schedule(""),
            Err(ScheduleParseError::Unrecognised(_))
        ));
    }

    #[test]
    fn schedule_serde_round_trips() {
        for s in [
            Schedule::Duration { seconds: 60 },
            parse_schedule("*/15 * * * *").unwrap(),
            Schedule::At {
                unix_seconds: 1_700_000_000,
            },
        ] {
            let json = serde_json::to_string(&s).unwrap();
            let back: Schedule = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }
}
