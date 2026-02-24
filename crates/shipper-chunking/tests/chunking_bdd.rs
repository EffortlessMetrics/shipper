use shipper_chunking::chunk_by_max_concurrent;

#[test]
fn bdd_given_no_items_when_chunking_then_returns_empty_batches() {
    // Given: an empty workload and a concurrency limit.
    let items: Vec<&str> = vec![];
    let max_concurrent = 3;

    // When: chunking is requested.
    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    // Then: there are no batches and no work units.
    assert!(chunks.is_empty());
}

#[test]
fn bdd_given_nine_items_and_limit_three_when_chunking_then_three_batches() {
    // Given: nine items and a limit of three.
    let items: Vec<String> = (1..=9).map(|index| format!("crate-{index}")).collect();
    let max_concurrent = 3;

    // When: chunking is requested.
    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    // Then: three batches preserve ordering and respect the limit.
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|chunk| chunk.len() <= max_concurrent));
    let flattened: Vec<String> = chunks.into_iter().flatten().collect();
    assert_eq!(flattened, items);
}

#[test]
fn bdd_given_max_concurrent_one_and_many_items_when_chunking_then_runs_serially() {
    // Given: many items and a limit of one.
    let items = vec![10, 11, 12, 13, 14];
    let max_concurrent = 1;

    // When: chunking is requested.
    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    // Then: each chunk contains one item in deterministic order.
    assert_eq!(chunks.len(), items.len());
    for (idx, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk, &[items[idx]]);
    }
}
