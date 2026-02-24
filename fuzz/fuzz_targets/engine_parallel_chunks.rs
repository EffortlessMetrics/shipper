#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_chunking::chunk_by_max_concurrent;

fuzz_target!(|data: (u8, Vec<u8>)| {
    let (max_hint, payload) = data;
    let max_concurrent = (max_hint as usize).max(1);

    let items: Vec<String> = payload
        .iter()
        .enumerate()
        .map(|(index, byte)| format!("{index}:{byte}"))
        .collect();

    let chunks = chunk_by_max_concurrent(&items, max_concurrent);

    assert!(chunks.iter().all(|chunk| chunk.len() <= max_concurrent));
    assert_eq!(
        chunks
            .iter()
            .flat_map(|chunk| chunk.iter())
            .collect::<Vec<_>>()
            .as_slice(),
        items.iter().collect::<Vec<_>>().as_slice()
    );
    assert_eq!(
        chunks.iter().map(|chunk| chunk.len()).sum::<usize>(),
        items.len()
    );
});
