//! Capture the actual detached core warning paths in isolated subprocesses.

use super::*;
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const SENTINELS: [&str; 6] = [
    "synthetic-user-404",
    "synthetic-password-404",
    "synthetic-path-404",
    "synthetic-query-404",
    "synthetic-signing-404",
    "synthetic-response-body-404",
];

#[test]
fn core_warnings_do_not_disclose_webhook_transport_secrets() {
    for api in ["client", "optional", "parallel"] {
        for case in ["response", "connection", "invalid"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "webhook::transport_privacy_tests::warning_child",
                    "--nocapture",
                ])
                .env("SHIPPER_WEBHOOK_WARNING_API", api)
                .env("SHIPPER_WEBHOOK_WARNING_CASE", case)
                .env("NO_PROXY", "127.0.0.1,localhost")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{api}/{case}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("[warn] webhook delivery failed (non-blocking)"),
                "missing actual warning: {stderr}"
            );
            let category = match case {
                "response" => "HTTP status 503",
                "connection" => "connection or TLS failure",
                _ => "invalid request",
            };
            for sentinel in SENTINELS {
                assert!(
                    !stderr.contains(sentinel),
                    "core warning disclosed {sentinel}: {stderr}"
                );
            }
            assert!(stderr.contains(category), "missing category: {stderr}");
        }
    }
}

#[test]
fn warning_child() {
    let Ok(case) = std::env::var("SHIPPER_WEBHOOK_WARNING_CASE") else {
        return;
    };
    let server = (case == "response").then(|| tiny_http::Server::http("127.0.0.1:0").unwrap());
    let address = if let Some(server) = &server {
        server.server_addr().to_string()
    } else if case == "invalid" {
        "127.0.0.1:synthetic-response-body-404".to_owned()
    } else {
        let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
        reserved.local_addr().unwrap().to_string()
    };
    let server_worker = server.map(|server| {
        thread::spawn(move || {
            let request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .expect("core webhook request");
            request
                .respond(
                    tiny_http::Response::from_string(SENTINELS.join(" ")).with_status_code(503),
                )
                .unwrap();
        })
    });
    let config = WebhookConfig {
        url: format!(
            "http://{}:{}@{address}/{}?token={}",
            SENTINELS[0], SENTINELS[1], SENTINELS[2], SENTINELS[3]
        ),
        secret: Some(SENTINELS[4].to_owned()),
        timeout_secs: 1,
        ..Default::default()
    };
    let event = WebhookEvent::PublishCompleted {
        plan_id: "completed-plan".to_owned(),
        total_packages: 1,
        success_count: 1,
        failure_count: 0,
        skipped_count: 0,
        result: "success".to_owned(),
    };
    match std::env::var("SHIPPER_WEBHOOK_WARNING_API")
        .unwrap()
        .as_str()
    {
        "client" => WebhookClient::new(&config).unwrap().send_event(event),
        "optional" => maybe_send_event(&config, event),
        "parallel" => {
            let event = serde_json::from_value(serde_json::to_value(event).unwrap()).unwrap();
            crate::engine::parallel::webhook::maybe_send_event(&config, event);
        }
        other => panic!("unexpected fixture API: {other}"),
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while active_test_deliveries() != 0 {
        assert!(Instant::now() < deadline, "delivery did not finish");
        thread::sleep(Duration::from_millis(10));
    }
    if let Some(worker) = server_worker {
        worker.join().unwrap();
    }
}
