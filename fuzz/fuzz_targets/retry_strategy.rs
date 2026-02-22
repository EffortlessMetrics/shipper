#![no_main]

use libfuzzer_sys::fuzz_target;
use shipper::retry::{calculate_delay, RetryStrategyConfig, RetryStrategyType};
use std::time::Duration;

fuzz_target!(|data: (u32, u8, u64, u64, u8)| {
    let (attempt, strategy_type, base_ms, max_ms, jitter_byte) = data;
    
    // Clamp values to reasonable ranges
    let attempt = attempt % 100 + 1; // 1-100
    let strategy = match strategy_type % 4 {
        0 => RetryStrategyType::Immediate,
        1 => RetryStrategyType::Exponential,
        2 => RetryStrategyType::Linear,
        _ => RetryStrategyType::Constant,
    };
    let base_delay = Duration::from_millis(base_ms % 10000 + 1); // 1-10000ms
    let max_delay = Duration::from_millis(max_ms % 300000 + 100); // 100-300000ms
    let jitter = (jitter_byte as f64) / 255.0; // 0.0-1.0
    
    let config = RetryStrategyConfig {
        strategy,
        max_attempts: 100,
        base_delay,
        max_delay,
        jitter,
    };
    
    let delay = calculate_delay(&config, attempt);
    
    // Invariants:
    // 1. Delay should never exceed max_delay
    assert!(delay <= max_delay || strategy == RetryStrategyType::Immediate);
    
    // 2. Immediate strategy should always return zero
    if strategy == RetryStrategyType::Immediate {
        assert_eq!(delay, Duration::ZERO);
    }
    
    // 3. Constant strategy should always return base_delay (possibly with jitter)
    if strategy == RetryStrategyType::Constant && jitter == 0.0 {
        assert_eq!(delay, base_delay.min(max_delay));
    }
});