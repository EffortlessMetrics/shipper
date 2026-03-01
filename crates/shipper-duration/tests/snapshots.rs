use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DurationHolder {
    #[serde(
        deserialize_with = "shipper_duration::deserialize_duration",
        serialize_with = "shipper_duration::serialize_duration"
    )]
    value: Duration,
}

// ---------------------------------------------------------------------------
// Human-readable formatting snapshots
// ---------------------------------------------------------------------------

#[test]
fn format_zero() {
    let formatted = humantime::format_duration(Duration::ZERO).to_string();
    insta::assert_snapshot!("format_zero", formatted);
}

#[test]
fn format_milliseconds() {
    let cases = [1, 50, 250, 500, 999];
    let output: Vec<String> = cases
        .iter()
        .map(|&ms| {
            let d = Duration::from_millis(ms);
            format!("{ms}ms => {}", humantime::format_duration(d))
        })
        .collect();
    insta::assert_snapshot!("format_milliseconds", output.join("\n"));
}

#[test]
fn format_seconds() {
    let cases = [1, 2, 5, 30, 59];
    let output: Vec<String> = cases
        .iter()
        .map(|&s| {
            let d = Duration::from_secs(s);
            format!("{s}s => {}", humantime::format_duration(d))
        })
        .collect();
    insta::assert_snapshot!("format_seconds", output.join("\n"));
}

#[test]
fn format_minutes() {
    let cases = [1, 2, 5, 30, 59];
    let output: Vec<String> = cases
        .iter()
        .map(|&m| {
            let d = Duration::from_secs(m * 60);
            format!("{m}m => {}", humantime::format_duration(d))
        })
        .collect();
    insta::assert_snapshot!("format_minutes", output.join("\n"));
}

#[test]
fn format_hours() {
    let cases = [1, 2, 12, 23];
    let output: Vec<String> = cases
        .iter()
        .map(|&h| {
            let d = Duration::from_secs(h * 3600);
            format!("{h}h => {}", humantime::format_duration(d))
        })
        .collect();
    insta::assert_snapshot!("format_hours", output.join("\n"));
}

#[test]
fn format_days() {
    let cases: &[u64] = &[1, 7, 30, 365];
    let output: Vec<String> = cases
        .iter()
        .map(|&d_val| {
            let d = Duration::from_secs(d_val * 86400);
            format!("{d_val}d => {}", humantime::format_duration(d))
        })
        .collect();
    insta::assert_snapshot!("format_days", output.join("\n"));
}

#[test]
fn format_mixed_durations() {
    let cases: &[(u64, &str)] = &[
        (90, "1m30s"),
        (3661, "1h1m1s"),
        (86400 + 3600 + 60 + 1, "1d1h1m1s"),
        (1500, "25m"),
    ];
    let output: Vec<String> = cases
        .iter()
        .map(|&(secs, label)| {
            let d = Duration::from_secs(secs);
            format!("{label} ({secs}s) => {}", humantime::format_duration(d))
        })
        .collect();
    insta::assert_snapshot!("format_mixed_durations", output.join("\n"));
}

// ---------------------------------------------------------------------------
// Serde format snapshots
// ---------------------------------------------------------------------------

#[test]
fn serde_json_serialize_millis() {
    let cases = [0, 1, 250, 1000, 5000, 60_000, 3_600_000, 86_400_000];
    let output: Vec<String> = cases
        .iter()
        .map(|&ms| {
            let holder = DurationHolder {
                value: Duration::from_millis(ms),
            };
            let json = serde_json::to_string_pretty(&holder).expect("serialize");
            format!("--- {ms}ms ---\n{json}")
        })
        .collect();
    insta::assert_snapshot!("serde_json_serialize_millis", output.join("\n"));
}

#[test]
fn serde_json_deserialize_from_integer() {
    let cases = [0, 100, 1500, 60_000];
    let output: Vec<String> = cases
        .iter()
        .map(|&ms| {
            let json = format!(r#"{{"value":{ms}}}"#);
            let holder: DurationHolder = serde_json::from_str(&json).expect("deserialize");
            format!("{ms} => {:?}", holder.value)
        })
        .collect();
    insta::assert_snapshot!("serde_json_deserialize_from_integer", output.join("\n"));
}

#[test]
fn serde_json_deserialize_from_string() {
    let cases = ["0ms", "250ms", "2s", "1m", "1h", "1day"];
    let output: Vec<String> = cases
        .iter()
        .map(|&s| {
            let json = format!(r#"{{"value":"{s}"}}"#);
            let holder: DurationHolder = serde_json::from_str(&json).expect("deserialize");
            format!("{s:>6} => {:?}", holder.value)
        })
        .collect();
    insta::assert_snapshot!("serde_json_deserialize_from_string", output.join("\n"));
}

#[test]
fn serde_toml_deserialize() {
    let cases = ["250ms", "2s", "5m", "1h"];
    let output: Vec<String> = cases
        .iter()
        .map(|&s| {
            let toml_str = format!(r#"value = "{s}""#);
            let holder: DurationHolder = toml::from_str(&toml_str).expect("toml deserialize");
            format!("{s:>5} => {:?}", holder.value)
        })
        .collect();
    insta::assert_snapshot!("serde_toml_deserialize", output.join("\n"));
}

// ---------------------------------------------------------------------------
// Error message snapshots
// ---------------------------------------------------------------------------

#[test]
fn parse_error_invalid_input() {
    let cases = ["not-a-duration", "", "abc", "1x", "ms"];
    let output: Vec<String> = cases
        .iter()
        .map(|&input| {
            let err = shipper_duration::parse_duration(input)
                .expect_err(&format!("should fail for {input:?}"));
            format!("{input:?} => {err}")
        })
        .collect();
    insta::assert_snapshot!("parse_error_invalid_input", output.join("\n"));
}

#[test]
fn serde_error_invalid_duration_string() {
    let cases = [
        r#"{"value":"not-a-duration"}"#,
        r#"{"value":"abc"}"#,
        r#"{"value":"1x"}"#,
    ];
    let output: Vec<String> = cases
        .iter()
        .map(|&json| {
            let err = serde_json::from_str::<DurationHolder>(json)
                .expect_err(&format!("should fail for {json}"));
            format!("{json}\n  => {err}")
        })
        .collect();
    insta::assert_snapshot!("serde_error_invalid_duration_string", output.join("\n"));
}
