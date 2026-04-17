use shipper_core::engine::parallel::chunk_by_max_concurrent;

#[test]
fn bdd_given_empty_input_when_chunking_then_returns_no_batches() {
    // Given: no items and any concurrency setting.
    let items: Vec<String> = vec![];
    let max_concurrent = 4;

    // When: chunking is requested.
    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    // Then: no batches are emitted.
    assert!(chunks.is_empty());
}

#[test]
fn bdd_given_limit_three_with_nine_items_then_returns_three_item_chunks() {
    // Given: nine packages and max concurrent 3.
    let items: Vec<String> = (1..=9).map(|n| format!("crate-{n}")).collect();
    let max_concurrent = 3;

    // When: chunking is requested.
    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    // Then: three chunks are produced, each with at most three entries, with order preserved.
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|chunk| chunk.len() <= max_concurrent));
    assert_eq!(
        chunks
            .iter()
            .flat_map(|chunk| chunk.iter())
            .collect::<Vec<_>>(),
        items.iter().collect::<Vec<_>>()
    );
}
