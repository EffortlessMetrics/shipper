# shipper-events

Event logging for shipper publish operations and audit trails.

Part of the [shipper](https://github.com/EffortlessMetrics/shipper) workspace — a publishing reliability layer for Rust workspaces.

## Overview

`shipper-events` provides a tiny event log API used by shipper internals:

- keep publish lifecycle events in memory (`EventLog`)
- persist events as append-only JSONL files
- load existing logs and filter by exact package id (`package` string)

## Data format

Events are stored in `.jsonl` (newline-delimited JSON).

- file name: `events.jsonl`
- each line is a serialized `shipper_types::PublishEvent` object
- writes are append-only

## Quick usage

```rust
use chrono::Utc;
use shipper_events::{EventLog, events_path};
use shipper_types::{EventType, PublishEvent};
use std::path::Path;

let mut log = EventLog::new();
log.record(PublishEvent {
    timestamp: Utc::now(),
    event_type: EventType::PackageStarted {
        name: "my-crate".to_string(),
        version: "1.0.0".to_string(),
    },
    package: "my-crate@1.0.0".to_string(),
});

let path = events_path(Path::new(".shipper"));
log.write_to_file(&path).expect("write events");

let loaded = EventLog::read_from_file(&path).expect("read events");
assert_eq!(loaded.len(), 1);
```

## License

MIT OR Apache-2.0
