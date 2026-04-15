use shipper::state::execution_state as state;
use shipper::store;

#[test]
fn state_and_store_accept_current_and_minimum_receipt_schema_versions() {
    state::validate_receipt_version(state::CURRENT_RECEIPT_VERSION).expect("state current");
    state::validate_receipt_version(state::MINIMUM_SUPPORTED_VERSION).expect("state minimum");

    store::validate_schema_version(state::CURRENT_RECEIPT_VERSION).expect("store current");
    store::validate_schema_version(state::MINIMUM_SUPPORTED_VERSION).expect("store minimum");
}

#[test]
fn state_and_store_reject_legacy_schema_versions() {
    let state_err =
        state::validate_receipt_version("shipper.receipt.v0").expect_err("state must fail");
    assert!(state_err.to_string().contains("too old"));

    let store_err =
        store::validate_schema_version("shipper.receipt.v0").expect_err("store must fail");
    assert!(store_err.to_string().contains("too old"));
}
