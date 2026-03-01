//! Duration parsing and serde codecs for shipper.
//!
//! This crate centralizes duration handling so CLI parsing and config/state
//! serde use one implementation.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serializer};

/// Parse a human-readable duration string (for example `2s`, `500ms`, `1m`).
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use shipper_duration::parse_duration;
///
/// assert_eq!(parse_duration("2s").unwrap(), Duration::from_secs(2));
/// assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
/// assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
/// ```
pub fn parse_duration(input: &str) -> Result<Duration, humantime::DurationError> {
    humantime::parse_duration(input)
}

/// Deserialize a [`Duration`] from either a human-readable string or a millisecond integer.
pub fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DurationHelper {
        String(String),
        U64(u64),
    }

    match DurationHelper::deserialize(deserializer)? {
        DurationHelper::String(s) => parse_duration(&s)
            .map_err(|e| serde::de::Error::custom(format!("invalid duration: {e}"))),
        DurationHelper::U64(ms) => Ok(Duration::from_millis(ms)),
    }
}

/// Serialize a [`Duration`] as milliseconds (`u64`) for stable round-tripping.
pub fn serialize_duration<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct DurationHolder {
        #[serde(
            deserialize_with = "deserialize_duration",
            serialize_with = "serialize_duration"
        )]
        value: Duration,
    }

    #[test]
    fn parse_duration_accepts_human_readable_values() {
        assert_eq!(
            parse_duration("250ms").expect("parse"),
            Duration::from_millis(250)
        );
        assert_eq!(parse_duration("2s").expect("parse"), Duration::from_secs(2));
    }

    #[test]
    fn deserialize_accepts_number_and_string() {
        let from_num: DurationHolder = serde_json::from_str(r#"{"value":1500}"#).expect("json");
        assert_eq!(from_num.value, Duration::from_millis(1500));

        let from_str: DurationHolder = serde_json::from_str(r#"{"value":"1500ms"}"#).expect("json");
        assert_eq!(from_str.value, Duration::from_millis(1500));
    }

    #[test]
    fn serialize_writes_milliseconds() {
        let value = DurationHolder {
            value: Duration::from_millis(4321),
        };
        let json = serde_json::to_value(&value).expect("json");
        assert_eq!(json["value"], 4321);
    }

    #[test]
    fn deserialize_rejects_invalid_duration_string() {
        let err = serde_json::from_str::<DurationHolder>(r#"{"value":"not-a-duration"}"#)
            .expect_err("must fail");
        assert!(err.to_string().contains("invalid duration"));
    }

    proptest! {
        #[test]
        fn duration_roundtrips_as_milliseconds(ms in 0_u64..10_000_000_000) {
            let holder = DurationHolder {
                value: Duration::from_millis(ms),
            };

            let json = serde_json::to_string(&holder).expect("serialize");
            let reparsed: DurationHolder = serde_json::from_str(&json).expect("deserialize");

            prop_assert_eq!(reparsed, holder);
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct DurationHolder {
        #[serde(
            deserialize_with = "deserialize_duration",
            serialize_with = "serialize_duration"
        )]
        value: Duration,
    }

    proptest! {
        /// Human-readable formatting always produces a non-empty string.
        #[test]
        fn format_is_never_empty(ms in 0u64..10_000_000_000u64) {
            let d = Duration::from_millis(ms);
            let formatted = humantime::format_duration(d).to_string();
            prop_assert!(!formatted.is_empty(), "formatted duration was empty for {ms}ms");
        }

        /// Formatting the same duration twice always yields the same string.
        #[test]
        fn format_consistency(ms in 0u64..10_000_000_000u64) {
            let d = Duration::from_millis(ms);
            let first = humantime::format_duration(d).to_string();
            let second = humantime::format_duration(d).to_string();
            prop_assert_eq!(first, second);
        }

        /// format → parse round-trip preserves the original duration.
        #[test]
        fn parse_format_roundtrip(ms in 0u64..10_000_000u64) {
            let d = Duration::from_millis(ms);
            let formatted = humantime::format_duration(d).to_string();
            let parsed = parse_duration(&formatted).expect("should parse formatted duration");
            prop_assert_eq!(parsed, d);
        }

        /// Sub-second durations mention "ms" in the formatted output.
        #[test]
        fn millisecond_range_contains_ms(ms in 1u64..1000u64) {
            let d = Duration::from_millis(ms);
            let formatted = humantime::format_duration(d).to_string();
            prop_assert!(formatted.contains("ms"), "expected 'ms' in \"{formatted}\"");
        }

        /// Whole-second durations (< 1 min) mention "s" in the formatted output.
        #[test]
        fn seconds_range_contains_s(secs in 1u64..60u64) {
            let d = Duration::from_secs(secs);
            let formatted = humantime::format_duration(d).to_string();
            prop_assert!(formatted.contains('s'), "expected 's' in \"{formatted}\"");
        }

        /// Whole-minute durations mention "m" in the formatted output.
        #[test]
        fn minutes_range_contains_m(mins in 1u64..60u64) {
            let d = Duration::from_secs(mins * 60);
            let formatted = humantime::format_duration(d).to_string();
            prop_assert!(formatted.contains('m'), "expected 'm' in \"{formatted}\"");
        }

        /// Whole-hour durations mention "h" in the formatted output.
        #[test]
        fn hours_range_contains_h(hours in 1u64..24u64) {
            let d = Duration::from_secs(hours * 3600);
            let formatted = humantime::format_duration(d).to_string();
            prop_assert!(formatted.contains('h'), "expected 'h' in \"{formatted}\"");
        }

        /// Serde JSON round-trip via integer millisecond representation.
        #[test]
        fn serde_json_u64_roundtrip(ms in 0u64..10_000_000_000u64) {
            let json = format!(r#"{{"value":{ms}}}"#);
            let holder: DurationHolder = serde_json::from_str(&json).expect("deserialize");
            prop_assert_eq!(holder.value, Duration::from_millis(ms));
        }

        /// Serde TOML round-trip via human-readable string representation.
        #[test]
        fn serde_toml_string_roundtrip(ms in 1u64..10_000_000u64) {
            let d = Duration::from_millis(ms);
            let formatted = humantime::format_duration(d).to_string();
            let toml_str = format!("value = \"{formatted}\"");
            let holder: DurationHolder = toml::from_str(&toml_str).expect("toml deserialize");
            prop_assert_eq!(holder.value, d);
        }
    }
}
