//! Duration parsing and serde codecs for shipper.
//!
//! This crate centralizes duration handling so CLI parsing and config/state
//! serde use one implementation.

use std::time::Duration;

use serde::{Deserialize, Deserializer, Serializer};

/// Parse a human-readable duration string (for example `2s`, `500ms`, `1m`).
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
