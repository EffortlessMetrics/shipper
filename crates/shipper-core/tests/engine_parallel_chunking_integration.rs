use shipper_core::engine::parallel::chunk_by_max_concurrent;

#[test]
fn integration_chunking_preserves_order_for_non_trivial_input() {
    let items = vec![
        "core".to_string(),
        "utils".to_string(),
        "service".to_string(),
        "api".to_string(),
        "cli".to_string(),
    ];
    let max_concurrent = 2;

    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0], vec!["core", "utils"]);
    assert_eq!(chunks[1], vec!["service", "api"]);
    assert_eq!(chunks[2], vec!["cli"]);

    let flat: Vec<&String> = chunks.iter().flatten().collect();
    assert_eq!(flat, items.iter().collect::<Vec<_>>());
}
