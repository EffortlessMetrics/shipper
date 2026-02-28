#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper_git::GitContext;

fuzz_target!(|data: &[u8]| {
    // Fuzz GitContext deserialization from arbitrary bytes
    if let Ok(input) = std::str::from_utf8(data) {
        // Attempt JSON deserialization — must not panic
        if let Ok(ctx) = serde_json::from_str::<GitContext>(input) {
            // Round-trip: serialize back and deserialize again
            if let Ok(json) = serde_json::to_string(&ctx) {
                let ctx2: GitContext =
                    serde_json::from_str(&json).expect("round-trip deserialization must succeed");
                assert_eq!(ctx.commit, ctx2.commit);
                assert_eq!(ctx.branch, ctx2.branch);
                assert_eq!(ctx.tag, ctx2.tag);
                assert_eq!(ctx.dirty, ctx2.dirty);
            }

            // Exercise accessor methods on deserialized context
            let _ = ctx.has_commit();
            let _ = ctx.is_dirty();
            let _ = ctx.short_commit();
        }

        // Treat input as a simulated commit hash and exercise short_commit
        let ctx_hash = GitContext {
            commit: Some(input.to_string()),
            branch: None,
            tag: None,
            dirty: None,
        };
        let _ = ctx_hash.short_commit();
        let _ = ctx_hash.has_commit();
        let _ = ctx_hash.is_dirty();

        // Treat input as a branch name
        let ctx_branch = GitContext {
            commit: None,
            branch: Some(input.to_string()),
            tag: None,
            dirty: None,
        };
        let _ = ctx_branch.has_commit();
        let _ = ctx_branch.is_dirty();
        let _ = ctx_branch.short_commit();
    }
});
