use chrono::{DateTime, Utc};
use serde_json::json;
use shipper_types::{EventType, PublishEvent};

fn attempted_event(max_attempts: u32) -> PublishEvent {
    PublishEvent {
        timestamp: "2026-10-29T12:00:00Z".parse::<DateTime<Utc>>().unwrap(),
        event_type: EventType::PackageAttempted {
            attempt: 1,
            command: "cargo publish --registry fixture".to_string(),
            max_attempts,
        },
        package: "fixture@1.0.0".to_string(),
    }
}

#[test]
fn package_attempted_omits_zero_max_attempts_on_the_wire() {
    let wire = serde_json::to_value(attempted_event(0)).expect("serialize event");
    assert_eq!(
        wire,
        json!({
            "timestamp": "2026-10-29T12:00:00Z",
            "event_type": {
                "type": "package_attempted",
                "attempt": 1,
                "command": "cargo publish --registry fixture"
            },
            "package": "fixture@1.0.0"
        })
    );
}

#[test]
fn package_attempted_preserves_nonzero_max_attempts_on_the_wire() {
    for max_attempts in [1, 6, u32::MAX] {
        let wire = serde_json::to_value(attempted_event(max_attempts)).expect("serialize event");
        assert_eq!(wire["event_type"]["max_attempts"], json!(max_attempts));
        let decoded: PublishEvent = serde_json::from_value(wire).expect("deserialize event");
        match decoded.event_type {
            EventType::PackageAttempted {
                attempt,
                command,
                max_attempts: decoded_max,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(command, "cargo publish --registry fixture");
                assert_eq!(decoded_max, max_attempts);
            }
            other => panic!("wrong event type: {other:?}"),
        }
    }
}

#[test]
fn legacy_package_attempted_without_max_attempts_defaults_to_zero() {
    // This literal represents the earlier event format, independently of the
    // current serializer, so removing the field's serde default is observable.
    let legacy = json!({
        "timestamp": "2026-10-29T12:00:00Z",
        "event_type": {
            "type": "package_attempted",
            "attempt": 2,
            "command": "cargo publish --registry fixture"
        },
        "package": "fixture@1.0.0"
    });
    let decoded: PublishEvent = serde_json::from_value(legacy.clone()).expect("read legacy event");
    assert!(matches!(
        &decoded.event_type,
        EventType::PackageAttempted {
            attempt: 2,
            max_attempts: 0,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(decoded).unwrap(), legacy);
}
