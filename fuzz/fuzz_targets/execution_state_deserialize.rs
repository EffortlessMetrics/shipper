#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_types::ExecutionState;

fuzz_target!(|data: &[u8]| {
    // Fuzz raw JSON deserialization of ExecutionState
    if let Ok(state) = serde_json::from_slice::<ExecutionState>(data) {
        // Roundtrip: serialize then deserialize again
        let json = serde_json::to_vec(&state).expect("serialize back");
        let rt: ExecutionState = serde_json::from_slice(&json).expect("roundtrip deserialize");

        // Verify structural invariants survive the roundtrip
        assert_eq!(state.packages.len(), rt.packages.len());
        assert_eq!(state.plan_id, rt.plan_id);
        assert_eq!(state.state_version, rt.state_version);
    }

    // Also try interpreting as UTF-8 string for serde_json::from_str path
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<ExecutionState>(text);
    }
});
