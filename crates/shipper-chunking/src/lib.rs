//! Chunking helpers for bounded-size parallel work batches.
//!
//! This crate isolates the "split work list by max concurrency" concern from the
//! parallel publish engine so it can be validated and fuzzed independently.

/// Split a list of items into contiguous chunks bounded by `max_concurrent`.
///
/// - `max_concurrent <= 0` is treated as `1`.
/// - Empty input returns an empty list of chunks.
/// - Item order is preserved across chunks.
///
/// # Examples
///
/// ```
/// use shipper_chunking::chunk_by_max_concurrent;
///
/// let items = vec!["a", "b", "c", "d", "e"];
/// let chunks = chunk_by_max_concurrent(&items, 2);
/// assert_eq!(chunks, vec![vec!["a", "b"], vec!["c", "d"], vec!["e"]]);
///
/// // Empty input returns no chunks
/// let empty: Vec<i32> = vec![];
/// assert!(chunk_by_max_concurrent(&empty, 3).is_empty());
/// ```
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

    #[test]
    fn chunking_max_concurrent_zero_treated_as_one() {
        let items = vec!["a", "b", "c"];
        let chunks = chunk_by_max_concurrent(&items, 0);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], vec!["a"]);
        assert_eq!(chunks[1], vec!["b"]);
        assert_eq!(chunks[2], vec!["c"]);
    }

    #[test]
    fn chunking_max_concurrent_usize_max() {
        let items = vec!["x", "y", "z"];
        let chunks = chunk_by_max_concurrent(&items, usize::MAX);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], vec!["x", "y", "z"]);
    }

    #[test]
    fn chunking_single_item_with_max_concurrent_one() {
        let items = vec!["only"];
        let chunks = chunk_by_max_concurrent(&items, 1);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], vec!["only"]);
    }

    #[test]
    fn chunking_large_list_100_items_by_7() {
        let items: Vec<i32> = (0..100).collect();
        let chunks = chunk_by_max_concurrent(&items, 7);

        // 100 / 7 = 14 full chunks of 7 + 1 chunk of 2 = 15 chunks
        assert_eq!(chunks.len(), 15);
        for chunk in &chunks[..14] {
            assert_eq!(chunk.len(), 7);
        }
        assert_eq!(chunks[14].len(), 2);

        let flattened: Vec<i32> = chunks.into_iter().flatten().collect();
        assert_eq!(flattened, items);
    }

    #[test]
    fn chunking_with_integer_types() {
        let items: Vec<u64> = vec![10, 20, 30, 40, 50, 60];
        let chunks = chunk_by_max_concurrent(&items, 4);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], vec![10, 20, 30, 40]);
        assert_eq!(chunks[1], vec![50, 60]);
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

        #[test]
        fn chunk_sizes_never_exceed_max_and_total_preserved(
            items in prop::collection::vec(0i32..1000, 0..200),
            max_concurrent in 0usize..32,
        ) {
            let effective = max_concurrent.max(1);
            let chunks = chunk_by_max_concurrent(&items, max_concurrent);

            // Every chunk respects the bound
            for chunk in &chunks {
                prop_assert!(chunk.len() <= effective);
                prop_assert!(!chunk.is_empty());
            }

            // Total items preserved
            let total: usize = chunks.iter().map(|c| c.len()).sum();
            prop_assert_eq!(total, items.len());

            // Order preserved
            let flattened: Vec<i32> = chunks.into_iter().flatten().collect();
            prop_assert_eq!(flattened, items);
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::chunk_by_max_concurrent;
    use insta::assert_yaml_snapshot;
    use insta::assert_debug_snapshot;

    #[test]
    fn snapshot_chunk_5_by_2() {
        let items = vec!["a", "b", "c", "d", "e"];
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 2));
    }

    #[test]
    fn snapshot_chunk_6_by_3() {
        let items = vec!["core", "utils", "api", "cli", "web", "docs"];
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 3));
    }

    #[test]
    fn snapshot_chunk_single_item() {
        let items = vec!["only"];
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 5));
    }

    #[test]
    fn snapshot_chunk_max_concurrent_1() {
        let items = vec!["a", "b", "c"];
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 1));
    }

    #[test]
    fn snapshot_chunk_exact_fit() {
        let items = vec!["x", "y", "z"];
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 3));
    }

    #[test]
    fn snapshot_chunk_empty() {
        let items: Vec<&str> = vec![];
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 4));
    }

    #[test]
    fn snapshot_chunk_max_concurrent_zero() {
        let items = vec!["a", "b", "c"];
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, 0));
    }

    #[test]
    fn snapshot_chunk_max_concurrent_usize_max() {
        let items = vec!["x", "y", "z"];
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, usize::MAX));
    }

    #[test]
    fn snapshot_chunk_single_item_max_one() {
        let items = vec!["solo"];
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, 1));
    }

    #[test]
    fn snapshot_chunk_large_list_by_7() {
        let items: Vec<i32> = (1..=21).collect();
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, 7));
    }
}
