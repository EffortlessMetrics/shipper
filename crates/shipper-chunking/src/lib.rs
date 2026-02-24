//! Chunking helpers for bounded-size parallel work batches.
//!
//! This crate isolates the "split work list by max concurrency" concern from the
//! parallel publish engine so it can be validated and fuzzed independently.

/// Split a list of items into contiguous chunks bounded by `max_concurrent`.
///
/// - `max_concurrent <= 0` is treated as `1`.
/// - Empty input returns an empty list of chunks.
/// - Item order is preserved across chunks.
pub fn chunk_by_max_concurrent<T: Clone>(items: &[T], max_concurrent: usize) -> Vec<Vec<T>> {
    let batch_size = max_concurrent.max(1);
    if items.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut index = 0usize;

    while index < items.len() {
        let next = (index + batch_size).min(items.len());
        chunks.push(items[index..next].to_vec());
        index = next;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::chunk_by_max_concurrent;

    #[test]
    fn chunking_empty_input_returns_no_batches() {
        let items: Vec<String> = vec![];
        let chunks = chunk_by_max_concurrent(&items, 4);
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunking_respects_max_concurrent() {
        let items = vec!["a", "b", "c", "d", "e"];
        let chunks = chunk_by_max_concurrent(&items, 2);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], vec!["a", "b"]);
        assert_eq!(chunks[1], vec!["c", "d"]);
        assert_eq!(chunks[2], vec!["e"]);
    }

    #[test]
    fn chunking_preserves_order_and_total_items() {
        let items = vec![1, 2, 3, 4, 5];
        let chunks = chunk_by_max_concurrent(&items, 10);
        let flattened: Vec<i32> = chunks
            .iter()
            .flat_map(|chunk| chunk.iter().cloned())
            .collect();

        assert_eq!(flattened, items);
        assert_eq!(chunks.len(), 1);
    }
}

#[cfg(test)]
mod property_tests {
    use super::chunk_by_max_concurrent;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn chunking_preserves_items_and_order(
            items in prop::collection::vec("[a-z]{0,6}", 0..128),
            max_concurrent in 1usize..16,
        ) {
            let chunks = chunk_by_max_concurrent(&items, max_concurrent);
            let flattened: Vec<String> = chunks
                .iter()
                .flat_map(|chunk| chunk.iter().cloned())
                .collect();

            prop_assert_eq!(flattened.len(), items.len());
            prop_assert_eq!(flattened, items.clone());

            let sum_len: usize = chunks.iter().map(|chunk| chunk.len()).sum();
            prop_assert_eq!(sum_len, items.len());
            for chunk in chunks {
                prop_assert!(chunk.len() <= max_concurrent.max(1));
            }
        }
    }
}
