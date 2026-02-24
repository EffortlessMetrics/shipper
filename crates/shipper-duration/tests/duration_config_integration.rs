use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RetryLikeConfig {
    #[serde(
        deserialize_with = "shipper_duration::deserialize_duration",
        serialize_with = "shipper_duration::serialize_duration"
    )]
    base_delay: Duration,
    #[serde(
        deserialize_with = "shipper_duration::deserialize_duration",
        serialize_with = "shipper_duration::serialize_duration"
    )]
    max_delay: Duration,
}

#[test]
fn toml_accepts_human_readable_durations() {
    let cfg: RetryLikeConfig = toml::from_str(
        r#"
base_delay = "250ms"
max_delay = "2s"
"#,
    )
    .expect("parse toml");

    assert_eq!(cfg.base_delay, Duration::from_millis(250));
    assert_eq!(cfg.max_delay, Duration::from_secs(2));
}

#[test]
fn json_accepts_millisecond_integers() {
    let cfg: RetryLikeConfig =
        serde_json::from_str(r#"{"base_delay":100,"max_delay":2500}"#).expect("parse json");

    assert_eq!(cfg.base_delay, Duration::from_millis(100));
    assert_eq!(cfg.max_delay, Duration::from_millis(2500));
}

#[test]
fn json_serializes_as_milliseconds() {
    let cfg = RetryLikeConfig {
        base_delay: Duration::from_millis(150),
        max_delay: Duration::from_secs(5),
    };

    let out = serde_json::to_string(&cfg).expect("serialize json");
    assert_eq!(out, r#"{"base_delay":150,"max_delay":5000}"#);
}
