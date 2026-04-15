#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper::state::events::EventLog;
use shipper_types::{EventType, PublishEvent};
use tempfile::tempdir;

// Fuzz that event log write/read roundtrips: events written to a JSONL
// file and read back must preserve count and content.
fuzz_target!(|data: &[u8]| {
    // 1. Fuzz deserialization of individual PublishEvent lines
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<PublishEvent>(text);
    }
    if let Ok(event) = serde_json::from_slice::<PublishEvent>(data) {
        let json = serde_json::to_string(&event).expect("serialize must succeed");
        let rt: PublishEvent =
            serde_json::from_str(&json).expect("roundtrip deserialize must succeed");
        assert_eq!(event.package, rt.package);
    }

    // 2. Fuzz the EventLog file write → read roundtrip with constructed events
    if data.len() < 2 {
        return;
    }

    let event_count = (data[0] as usize % 8) + 1;
    let mut log = EventLog::new();

    for i in 0..event_count {
        let byte = data.get(1 + i).copied().unwrap_or(0);
        let event = make_event(byte, i);
        log.record(event);
    }

    assert_eq!(log.len(), event_count);
    assert!(!log.is_empty());

    let td = match tempdir() {
        Ok(v) => v,
        Err(_) => return,
    };
    let path = td.path().join("events.jsonl");

    if log.write_to_file(&path).is_ok() {
        let loaded = EventLog::read_from_file(&path).expect("read must succeed after write");
        assert_eq!(
            loaded.len(),
            event_count,
            "event count must survive roundtrip"
        );
    }
});

fn make_event(byte: u8, index: usize) -> PublishEvent {
    let event_type = match byte % 4 {
        0 => EventType::PlanCreated {
            plan_id: format!("fuzz-{index}"),
            package_count: index,
        },
        1 => EventType::ExecutionStarted,
        2 => EventType::PackageStarted {
            name: format!("pkg-{index}"),
            version: format!("{}.0.0", index),
        },
        _ => EventType::PackageSkipped {
            reason: format!("fuzz-reason-{index}"),
        },
    };

    PublishEvent {
        timestamp: chrono::Utc::now(),
        event_type,
        package: format!("pkg-{index}@{index}.0.0"),
    }
}
