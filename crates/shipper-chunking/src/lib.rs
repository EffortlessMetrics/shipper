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

    // --- Edge-case: empty input variants ---

    #[test]
    fn empty_i32_slice_returns_empty_chunks() {
        let items: &[i32] = &[];
        let chunks = chunk_by_max_concurrent(items, 1);
        assert!(chunks.is_empty());
    }

    #[test]
    fn empty_input_with_large_max_concurrent() {
        let items: Vec<&str> = vec![];
        let chunks = chunk_by_max_concurrent(&items, 1000);
        assert!(chunks.is_empty());
    }

    // --- Edge-case: single item ---

    #[test]
    fn single_item_with_large_max_concurrent() {
        let items = vec![42];
        let chunks = chunk_by_max_concurrent(&items, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], vec![42]);
    }

    #[test]
    fn single_item_with_max_concurrent_zero() {
        let items = vec!["alone"];
        let chunks = chunk_by_max_concurrent(&items, 0);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], vec!["alone"]);
    }

    // --- Edge-case: chunk size larger than input ---

    #[test]
    fn chunk_size_much_larger_than_input() {
        let items = vec![1, 2, 3];
        let chunks = chunk_by_max_concurrent(&items, 999);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], vec![1, 2, 3]);
    }

    #[test]
    fn chunk_size_exactly_input_length() {
        let items = vec!["a", "b", "c", "d"];
        let chunks = chunk_by_max_concurrent(&items, 4);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], vec!["a", "b", "c", "d"]);
    }

    // --- Edge-case: chunk size of 1 ---

    #[test]
    fn chunk_size_one_produces_individual_chunks() {
        let items = vec!["x", "y", "z", "w"];
        let chunks = chunk_by_max_concurrent(&items, 1);
        assert_eq!(chunks.len(), 4);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.len(), 1);
            assert_eq!(chunk[0], items[i]);
        }
    }

    // --- Edge-case: large input (1000 items) ---

    #[test]
    fn large_input_1000_items_by_3() {
        let items: Vec<i32> = (0..1000).collect();
        let chunks = chunk_by_max_concurrent(&items, 3);
        // ceil(1000 / 3) = 334 chunks
        assert_eq!(chunks.len(), 334);
        for chunk in &chunks[..333] {
            assert_eq!(chunk.len(), 3);
        }
        assert_eq!(chunks[333].len(), 1);
        let flattened: Vec<i32> = chunks.into_iter().flatten().collect();
        assert_eq!(flattened, items);
    }

    #[test]
    fn large_input_1000_items_by_1() {
        let items: Vec<i32> = (0..1000).collect();
        let chunks = chunk_by_max_concurrent(&items, 1);
        assert_eq!(chunks.len(), 1000);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk, &[i as i32]);
        }
    }

    #[test]
    fn large_input_1000_items_by_1000() {
        let items: Vec<i32> = (0..1000).collect();
        let chunks = chunk_by_max_concurrent(&items, 1000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1000);
    }

    #[test]
    fn large_input_1000_items_by_17() {
        let items: Vec<i32> = (0..1000).collect();
        let chunks = chunk_by_max_concurrent(&items, 17);
        // ceil(1000 / 17) = 59 chunks
        assert_eq!(chunks.len(), 59);
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, 1000);
        let flattened: Vec<i32> = chunks.into_iter().flatten().collect();
        assert_eq!(flattened, items);
    }

    // --- Edge-case: items with dependency-like structure spanning chunks ---

    #[test]
    fn dependency_items_spanning_multiple_chunks() {
        // Simulate crate items where some depend on earlier ones
        #[derive(Debug, Clone, PartialEq)]
        struct CrateItem {
            name: &'static str,
            depends_on: Vec<&'static str>,
        }

        let items = vec![
            CrateItem {
                name: "core",
                depends_on: vec![],
            },
            CrateItem {
                name: "utils",
                depends_on: vec!["core"],
            },
            CrateItem {
                name: "api",
                depends_on: vec!["core", "utils"],
            },
            CrateItem {
                name: "cli",
                depends_on: vec!["api"],
            },
            CrateItem {
                name: "web",
                depends_on: vec!["api", "utils"],
            },
        ];

        let chunks = chunk_by_max_concurrent(&items, 2);
        assert_eq!(chunks.len(), 3);

        // "core" and "utils" in first chunk — "utils" depends on "core" from same chunk
        assert_eq!(chunks[0][0].name, "core");
        assert_eq!(chunks[0][1].name, "utils");

        // "api" depends on items in prior chunk, "cli" depends on "api" in same chunk
        assert_eq!(chunks[1][0].name, "api");
        assert_eq!(chunks[1][1].name, "cli");

        // "web" depends on items from both prior chunks
        assert_eq!(chunks[2][0].name, "web");
    }

    #[test]
    fn dependency_items_all_in_one_chunk() {
        #[derive(Debug, Clone, PartialEq)]
        struct CrateItem {
            name: &'static str,
            depends_on: Vec<&'static str>,
        }

        let items = vec![
            CrateItem {
                name: "core",
                depends_on: vec![],
            },
            CrateItem {
                name: "utils",
                depends_on: vec!["core"],
            },
            CrateItem {
                name: "api",
                depends_on: vec!["core", "utils"],
            },
        ];

        let chunks = chunk_by_max_concurrent(&items, 10);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 3);
        // All dependencies satisfied within the single chunk
        assert_eq!(chunks[0][0].name, "core");
        assert_eq!(chunks[0][2].name, "api");
    }

    // --- Ordering within chunks is preserved ---

    #[test]
    fn ordering_within_each_chunk_matches_input() {
        let items: Vec<i32> = (0..20).collect();
        let chunks = chunk_by_max_concurrent(&items, 4);

        let mut offset = 0;
        for chunk in &chunks {
            for (j, &item) in chunk.iter().enumerate() {
                assert_eq!(item, items[offset + j], "mismatch at offset {offset}+{j}");
            }
            offset += chunk.len();
        }
        assert_eq!(offset, items.len());
    }

    #[test]
    fn ordering_preserved_with_string_items() {
        let items: Vec<String> = (0..15).map(|i| format!("crate-{i}")).collect();
        let chunks = chunk_by_max_concurrent(&items, 4);

        let flattened: Vec<String> = chunks.into_iter().flatten().collect();
        assert_eq!(flattened, items);
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

        #[test]
        fn total_items_across_chunks_equals_input_length(
            items in prop::collection::vec(0u32..500, 0..300),
            max_concurrent in 1usize..64,
        ) {
            let chunks = chunk_by_max_concurrent(&items, max_concurrent);
            let total: usize = chunks.iter().map(|c| c.len()).sum();
            prop_assert_eq!(total, items.len());
        }

        #[test]
        fn chunk_count_is_ceil_of_n_over_chunk_size(
            n in 0usize..500,
            max_concurrent in 1usize..64,
        ) {
            let items: Vec<usize> = (0..n).collect();
            let chunks = chunk_by_max_concurrent(&items, max_concurrent);

            let expected_count = if n == 0 {
                0
            } else {
                (n + max_concurrent - 1) / max_concurrent
            };
            prop_assert_eq!(chunks.len(), expected_count);
        }

        #[test]
        fn ordering_within_chunks_preserved_property(
            items in prop::collection::vec(any::<i64>(), 0..150),
            max_concurrent in 1usize..20,
        ) {
            let chunks = chunk_by_max_concurrent(&items, max_concurrent);
            let mut offset = 0usize;
            for chunk in &chunks {
                for (j, item) in chunk.iter().enumerate() {
                    prop_assert_eq!(item, &items[offset + j]);
                }
                offset += chunk.len();
            }
            prop_assert_eq!(offset, items.len());
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::chunk_by_max_concurrent;
    use insta::assert_debug_snapshot;
    use insta::assert_yaml_snapshot;

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

    #[test]
    fn snapshot_chunk_1000_items_by_10() {
        let items: Vec<i32> = (0..1000).collect();
        let chunks = chunk_by_max_concurrent(&items, 10);
        // Snapshot only the chunk lengths to keep it readable
        let lens: Vec<usize> = chunks.iter().map(|c| c.len()).collect();
        assert_debug_snapshot!(lens);
    }

    #[test]
    fn snapshot_chunk_7_items_by_3() {
        let items = vec![
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
        ];
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, 3));
    }

    #[test]
    fn snapshot_chunk_size_1_with_4_items() {
        let items = vec!["w", "x", "y", "z"];
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, 1));
    }

    #[test]
    fn snapshot_chunk_10_items_by_5() {
        let items: Vec<i32> = (1..=10).collect();
        assert_yaml_snapshot!(chunk_by_max_concurrent(&items, 5));
    }

    #[test]
    fn snapshot_dependency_like_items_by_2() {
        let items = vec![
            ("core", vec![]),
            ("utils", vec!["core"]),
            ("api", vec!["core", "utils"]),
            ("cli", vec!["api"]),
            ("web", vec!["api", "utils"]),
        ];
        assert_debug_snapshot!(chunk_by_max_concurrent(&items, 2));
    }
}
