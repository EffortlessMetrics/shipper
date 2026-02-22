#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper::types::*;
use std::collections::BTreeMap;

fuzz_target!(|data: &[u8]| {
    // Try to parse as JSON and verify serialization roundtrips
    if let Ok(json_str) = std::str::from_utf8(data) {
        // Test ExecutionState roundtrip
        if let Ok(state) = serde_json::from_str::<ExecutionState>(json_str) {
            if let Ok(roundtripped) = serde_json::to_string(&state) {
                if let Ok(parsed) = serde_json::from_str::<ExecutionState>(&roundtripped) {
                    assert_eq!(state.plan_id, parsed.plan_id);
                    assert_eq!(state.packages.len(), parsed.packages.len());
                }
            }
        }
        
        // Test PreflightReport roundtrip
        if let Ok(report) = serde_json::from_str::<PreflightReport>(json_str) {
            if let Ok(roundtripped) = serde_json::to_string(&report) {
                if let Ok(parsed) = serde_json::from_str::<PreflightReport>(&roundtripped) {
                    assert_eq!(report.plan_id, parsed.plan_id);
                }
            }
        }
        
        // Test Receipt roundtrip
        if let Ok(receipt) = serde_json::from_str::<Receipt>(json_str) {
            if let Ok(roundtripped) = serde_json::to_string(&receipt) {
                if let Ok(parsed) = serde_json::from_str::<Receipt>(&roundtripped) {
                    assert_eq!(receipt.plan_id, parsed.plan_id);
                }
            }
        }
    }
});