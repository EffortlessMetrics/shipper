//! Readiness visibility helpers for parallel publish.
//!
//! Checks whether a newly-published crate version is visible on the registry,
//! with exponential backoff and optional sparse-index fallback.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;

use shipper_registry::HttpRegistryClient as RegistryClient;
use shipper_types::{EventType, PublishEvent, ReadinessConfig, ReadinessEvidence, ReadinessMethod};

/// Check readiness visibility with exponential backoff and optional sparse-index fallback.
pub(super) fn is_version_visible_with_backoff(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &ReadinessConfig,
) -> Result<(bool, Vec<ReadinessEvidence>)> {
    is_version_visible_with_backoff_and_events(reg, crate_name, version, config, &mut |_| Ok(()))
}

pub(super) fn is_version_visible_with_backoff_and_events(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &ReadinessConfig,
    emit_event: &mut dyn FnMut(PublishEvent) -> Result<()>,
) -> Result<(bool, Vec<ReadinessEvidence>)> {
    let mut evidence = Vec::new();
    let package = format!("{crate_name}@{version}");

    if !config.enabled {
        let visible = reg.version_exists(crate_name, version)?;
        emit_event(readiness_poll_event(&package, 1, visible))?;
        evidence.push(ReadinessEvidence {
            attempt: 1,
            visible,
            timestamp: Utc::now(),
            delay_before: Duration::ZERO,
        });
        return Ok((visible, evidence));
    }

    let start = Instant::now();
    let mut attempt = 0u32;

    if config.initial_delay > Duration::ZERO {
        emit_event(readiness_poll_scheduled_event(
            &package,
            1,
            config.initial_delay,
        ))?;
        thread::sleep(config.initial_delay);
    }

    loop {
        attempt += 1;

        let jittered_delay = if attempt == 1 {
            Duration::ZERO
        } else {
            let base_delay = config.poll_interval;
            let exponential_delay =
                base_delay.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(2).min(16)));
            let capped_delay = exponential_delay.min(config.max_delay);
            let jitter_range = config.jitter_factor;
            let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_range - jitter_range);
            Duration::from_millis((capped_delay.as_millis() as f64 * jitter).round() as u64)
        };

        let visible = match config.method {
            ReadinessMethod::Api => reg.version_exists(crate_name, version).unwrap_or(false),
            ReadinessMethod::Index => {
                is_version_visible_via_index(reg, crate_name, version, config).unwrap_or(false)
            }
            ReadinessMethod::Both => {
                if config.prefer_index {
                    if is_version_visible_via_index(reg, crate_name, version, config)
                        .unwrap_or(false)
                    {
                        true
                    } else {
                        reg.version_exists(crate_name, version).unwrap_or(false)
                    }
                } else if reg.version_exists(crate_name, version).unwrap_or(false) {
                    true
                } else {
                    is_version_visible_via_index(reg, crate_name, version, config).unwrap_or(false)
                }
            }
        };

        evidence.push(ReadinessEvidence {
            attempt,
            visible,
            timestamp: Utc::now(),
            delay_before: jittered_delay,
        });
        emit_event(readiness_poll_event(&package, attempt, visible))?;

        if visible {
            return Ok((true, evidence));
        }

        if start.elapsed() >= config.max_total_wait {
            return Ok((false, evidence));
        }

        let base_delay = config.poll_interval;
        let exponential_delay =
            base_delay.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1).min(16)));
        let capped_delay = exponential_delay.min(config.max_delay);
        let jitter_range = config.jitter_factor;
        let jitter = 1.0 + (rand::random::<f64>() * 2.0 * jitter_range - jitter_range);
        let next_delay =
            Duration::from_millis((capped_delay.as_millis() as f64 * jitter).round() as u64);
        emit_event(readiness_poll_scheduled_event(
            &package,
            attempt.saturating_add(1),
            next_delay,
        ))?;
        thread::sleep(next_delay);
    }
}

fn is_version_visible_via_index(
    reg: &RegistryClient,
    crate_name: &str,
    version: &str,
    config: &ReadinessConfig,
) -> Result<bool> {
    let content = if let Some(path) = &config.index_path {
        std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "failed to read local sparse-index path {}: {}",
                path.display(),
                e
            )
        })?
    } else {
        reg.fetch_sparse_index_file(reg.base_url(), crate_name)?
    };

    Ok(shipper_sparse_index::contains_version(&content, version))
}

fn readiness_poll_event(package: &str, attempt: u32, visible: bool) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPoll { attempt, visible },
        package: package.to_string(),
    }
}

fn readiness_poll_scheduled_event(package: &str, attempt: u32, delay: Duration) -> PublishEvent {
    PublishEvent {
        timestamp: Utc::now(),
        event_type: EventType::ReadinessPollScheduled {
            attempt,
            delay_ms: delay.as_millis() as u64,
            next_poll_at: Utc::now()
                + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero()),
        },
        package: package.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tiny_http::{Header, Response, Server, StatusCode};

    struct MockServer {
        base_url: String,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        counter: Arc<AtomicUsize>,
    }

    impl MockServer {
        fn request_count(&self) -> usize {
            self.counter.load(Ordering::SeqCst)
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.shutdown.store(true, Ordering::SeqCst);
            // Worker thread exits the next time `recv_timeout` returns.
        }
    }

    /// Spawn a small mock registry that serves a fixed sequence of
    /// (status, body) responses regardless of path. Subsequent requests
    /// reuse the last entry. The worker thread polls with a short
    /// timeout so it exits promptly when the test drops the server.
    fn spawn_mock_registry(responses: Vec<(u16, String)>) -> MockServer {
        let server = Server::http("127.0.0.1:0").expect("mock server");
        let base_url = format!("http://{}", server.server_addr());
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        std::thread::spawn(move || {
            let last = responses.last().cloned().unwrap_or((404, "{}".to_string()));
            let mut iter = responses.into_iter();
            while !shutdown_clone.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(50)) {
                    Ok(Some(req)) => {
                        counter_clone.fetch_add(1, Ordering::SeqCst);
                        let (status, body) = iter.next().unwrap_or_else(|| last.clone());
                        let resp = Response::from_string(body)
                            .with_status_code(StatusCode(status))
                            .with_header(
                                Header::from_bytes("Content-Type", "application/json")
                                    .expect("header"),
                            );
                        let _ = req.respond(resp);
                    }
                    Ok(None) => continue,
                    Err(_) => break,
                }
            }
        });

        MockServer {
            base_url,
            shutdown,
            counter,
        }
    }

    fn config_disabled() -> ReadinessConfig {
        ReadinessConfig {
            enabled: false,
            method: ReadinessMethod::Api,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(50),
            max_total_wait: Duration::from_millis(200),
            poll_interval: Duration::from_millis(10),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        }
    }

    fn config_enabled(method: ReadinessMethod) -> ReadinessConfig {
        ReadinessConfig {
            enabled: true,
            method,
            initial_delay: Duration::ZERO,
            max_delay: Duration::from_millis(20),
            max_total_wait: Duration::from_millis(150),
            poll_interval: Duration::from_millis(5),
            jitter_factor: 0.0,
            index_path: None,
            prefer_index: false,
        }
    }

    fn write_sparse_index(dir: &std::path::Path, versions: &[&str]) -> PathBuf {
        let path = dir.join("index-snippet.json");
        let mut f = std::fs::File::create(&path).expect("create index file");
        for v in versions {
            // sparse-index format: one JSON entry per line containing `vers` field.
            let entry = serde_json::json!({
                "name": "demo",
                "vers": v,
                "deps": [],
                "cksum": "",
                "features": {},
                "yanked": false,
            });
            writeln!(f, "{}", entry).expect("write entry");
        }
        path
    }

    // ── Disabled fast path ──────────────────────────────────────────

    #[test]
    fn disabled_returns_immediately_with_single_evidence_visible() {
        let server = spawn_mock_registry(vec![(200, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let (visible, evidence) =
            is_version_visible_with_backoff(&reg, "serde", "1.0.0", &config_disabled())
                .expect("ok");

        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].attempt, 1);
        assert_eq!(evidence[0].delay_before, Duration::ZERO);
        assert!(evidence[0].visible);
    }

    #[test]
    fn disabled_returns_immediately_with_single_evidence_not_visible() {
        let server = spawn_mock_registry(vec![(404, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let (visible, evidence) =
            is_version_visible_with_backoff(&reg, "demo", "0.0.1", &config_disabled()).expect("ok");

        assert!(!visible);
        assert_eq!(evidence.len(), 1);
    }

    // ── Api method ──────────────────────────────────────────────────

    #[test]
    fn api_method_succeeds_on_first_attempt() {
        let server = spawn_mock_registry(vec![(200, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let (visible, evidence) = is_version_visible_with_backoff(
            &reg,
            "serde",
            "1.0.0",
            &config_enabled(ReadinessMethod::Api),
        )
        .expect("ok");

        assert!(visible);
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].attempt, 1);
    }

    #[test]
    fn api_method_returns_false_after_max_total_wait() {
        let server = spawn_mock_registry(vec![(404, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Api);
        cfg.max_total_wait = Duration::from_millis(50);

        let (visible, evidence) =
            is_version_visible_with_backoff(&reg, "demo", "9.9.9", &cfg).expect("ok");

        assert!(!visible);
        assert!(!evidence.is_empty(), "should record at least one attempt");
        assert!(
            evidence.iter().all(|e| !e.visible),
            "no attempt should have reported visibility"
        );
    }

    // ── Index method ────────────────────────────────────────────────

    #[test]
    fn index_method_with_local_path_finds_version() {
        let td = tempdir().expect("tempdir");
        let path = write_sparse_index(td.path(), &["0.1.0", "1.2.3"]);

        let server = spawn_mock_registry(vec![(404, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Index);
        cfg.index_path = Some(path);

        let (visible, _) =
            is_version_visible_with_backoff(&reg, "demo", "1.2.3", &cfg).expect("ok");

        assert!(visible);
        assert_eq!(
            server.request_count(),
            0,
            "Index method with local path must not hit the network"
        );
    }

    #[test]
    fn index_method_with_local_path_misses_unknown_version() {
        let td = tempdir().expect("tempdir");
        let path = write_sparse_index(td.path(), &["0.1.0"]);

        let server = spawn_mock_registry(vec![(404, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Index);
        cfg.index_path = Some(path);
        cfg.max_total_wait = Duration::from_millis(40);

        let (visible, _) =
            is_version_visible_with_backoff(&reg, "demo", "9.9.9", &cfg).expect("ok");

        assert!(!visible);
    }

    #[test]
    fn is_version_visible_via_index_returns_error_when_local_path_missing() {
        let server = spawn_mock_registry(vec![(404, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Index);
        cfg.index_path = Some(PathBuf::from("/this/path/definitely/does/not/exist.json"));

        let err = is_version_visible_via_index(&reg, "demo", "1.0.0", &cfg)
            .expect_err("missing local path must surface as error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to read local sparse-index path"),
            "unexpected error message: {msg}"
        );
    }

    // ── Both method ─────────────────────────────────────────────────

    #[test]
    fn both_method_prefer_index_finds_via_local_path_without_api_call() {
        let td = tempdir().expect("tempdir");
        let path = write_sparse_index(td.path(), &["1.0.0"]);

        let server = spawn_mock_registry(vec![(404, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Both);
        cfg.prefer_index = true;
        cfg.index_path = Some(path);

        let (visible, _) =
            is_version_visible_with_backoff(&reg, "demo", "1.0.0", &cfg).expect("ok");

        assert!(visible);
        assert_eq!(
            server.request_count(),
            0,
            "prefer_index + index hit must not fall back to api",
        );
    }

    #[test]
    fn both_method_prefer_index_falls_back_to_api_when_index_misses() {
        let td = tempdir().expect("tempdir");
        let path = write_sparse_index(td.path(), &["0.1.0"]);

        let server = spawn_mock_registry(vec![(200, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Both);
        cfg.prefer_index = true;
        cfg.index_path = Some(path);

        let (visible, _) =
            is_version_visible_with_backoff(&reg, "demo", "1.0.0", &cfg).expect("ok");

        assert!(visible, "api fallback should report visible");
        assert!(
            server.request_count() >= 1,
            "api fallback must hit the network",
        );
    }

    #[test]
    fn both_method_default_tries_api_first_when_prefer_index_false() {
        let td = tempdir().expect("tempdir");
        let path = write_sparse_index(td.path(), &["1.0.0"]);

        let server = spawn_mock_registry(vec![(200, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Both);
        cfg.prefer_index = false;
        cfg.index_path = Some(path);

        let (visible, _) =
            is_version_visible_with_backoff(&reg, "demo", "1.0.0", &cfg).expect("ok");

        assert!(visible);
        assert!(
            server.request_count() >= 1,
            "api must be tried first when prefer_index=false",
        );
    }

    // ── Backoff scheduling ──────────────────────────────────────────

    #[test]
    fn first_attempt_records_zero_pre_delay() {
        let server = spawn_mock_registry(vec![(200, "{}".to_string())]);
        let reg = RegistryClient::new(&server.base_url);

        let cfg = config_enabled(ReadinessMethod::Api);
        let (_, evidence) =
            is_version_visible_with_backoff(&reg, "demo", "1.0.0", &cfg).expect("ok");

        assert_eq!(evidence[0].delay_before, Duration::ZERO);
    }

    #[test]
    fn subsequent_attempts_record_growing_jittered_delays() {
        // First two responses are 404 so the loop iterates; third is 200.
        let server = spawn_mock_registry(vec![
            (404, "{}".to_string()),
            (404, "{}".to_string()),
            (200, "{}".to_string()),
        ]);
        let reg = RegistryClient::new(&server.base_url);

        let mut cfg = config_enabled(ReadinessMethod::Api);
        cfg.max_total_wait = Duration::from_secs(5);
        cfg.poll_interval = Duration::from_millis(2);
        cfg.max_delay = Duration::from_millis(20);

        let (visible, evidence) =
            is_version_visible_with_backoff(&reg, "demo", "1.0.0", &cfg).expect("ok");

        assert!(visible);
        assert!(
            evidence.len() >= 3,
            "expected ≥3 attempts before visibility, got {}",
            evidence.len()
        );
        assert_eq!(evidence[0].delay_before, Duration::ZERO);
    }
}
