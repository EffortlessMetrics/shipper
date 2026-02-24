#[derive(Debug, Clone, PartialEq, Eq)]
struct PublishWave {
    package: String,
    version: String,
}

#[test]
fn chunking_contract_uses_realistic_parallel_workloads() {
    let items = vec![
        PublishWave {
            package: "base".into(),
            version: "0.1.0".into(),
        },
        PublishWave {
            package: "api".into(),
            version: "0.1.0".into(),
        },
        PublishWave {
            package: "cli".into(),
            version: "0.1.0".into(),
        },
        PublishWave {
            package: "desktop".into(),
            version: "0.1.0".into(),
        },
        PublishWave {
            package: "app".into(),
            version: "0.1.0".into(),
        },
    ];

    let chunks = shipper_chunking::chunk_by_max_concurrent(&items, 2);

    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].len(), 2);
    assert_eq!(chunks[1].len(), 2);
    assert_eq!(chunks[2].len(), 1);
    assert_eq!(chunks[0][0].package, "base");
    assert_eq!(chunks[0][1].package, "api");
    assert_eq!(chunks[2][0].package, "app");
    assert_eq!(chunks.iter().flat_map(|chunk| chunk.iter()).count(), items.len());
}

