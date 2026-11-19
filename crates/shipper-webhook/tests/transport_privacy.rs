//! Exercise real local transport failures, including every public error format.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use shipper_webhook::{WebhookConfig, WebhookPayload, send_webhook, send_webhook_async};

const USER: &str = "synthetic-user-404";
const PASSWORD: &str = "synthetic-password-404";
const PATH_SECRET: &str = "synthetic-path-404";
const QUERY_SECRET: &str = "synthetic-query-404";
const SIGNING_SECRET: &str = "synthetic-signing-404";
const BODY_SECRET: &str = "synthetic-response-body-404";

fn config(url: String) -> WebhookConfig {
    WebhookConfig {
        url,
        timeout_secs: 1,
        secret: Some(SIGNING_SECRET.to_owned()),
        ..Default::default()
    }
}

fn private_url(address: impl std::fmt::Display) -> String {
    format!("http://{USER}:{PASSWORD}@{address}/{PATH_SECRET}?token={QUERY_SECRET}")
}

fn assert_private(error: &anyhow::Error, category: &str) {
    let formats = [
        error.to_string(),
        format!("{error:#}"),
        format!("{error:?}"),
        format!("{error:#?}"),
        error
            .chain()
            .map(|cause| format!("{cause:?}: {cause}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ];
    for rendered in formats {
        assert!(
            rendered.contains(category),
            "missing safe category: {rendered}"
        );
        for sentinel in [
            USER,
            PASSWORD,
            PATH_SECRET,
            QUERY_SECRET,
            SIGNING_SECRET,
            BODY_SECRET,
        ] {
            assert!(
                !rendered.contains(sentinel),
                "transport error disclosed {sentinel}: {rendered}"
            );
        }
    }
}

fn status_server(status: u16) -> (WebhookConfig, thread::JoinHandle<()>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let cfg = config(private_url(server.server_addr()));
    let worker = thread::spawn(move || {
        let request = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("webhook request");
        assert!(request.url().contains(PATH_SECRET));
        assert!(
            request
                .headers()
                .iter()
                .any(|header| header.field.equiv("X-Hub-Signature-256"))
        );
        request
            .respond(
                tiny_http::Response::from_string(format!("{BODY_SECRET} {SIGNING_SECRET}"))
                    .with_status_code(status),
            )
            .unwrap();
    });
    (cfg, worker)
}

#[test]
fn sync_http_errors_keep_status_without_response_or_endpoint_secrets() {
    for status in [401, 422, 500, 503] {
        let (cfg, worker) = status_server(status);
        let error = send_webhook(&cfg, &WebhookPayload::default()).unwrap_err();
        worker.join().unwrap();
        assert_private(&error, &status.to_string());
    }
}

#[tokio::test]
async fn async_http_errors_keep_status_without_response_or_endpoint_secrets() {
    for status in [401, 422, 500, 503] {
        let (cfg, worker) = status_server(status);
        let error = send_webhook_async(&cfg, &WebhookPayload::default())
            .await
            .unwrap_err();
        worker.join().unwrap();
        assert_private(&error, &status.to_string());
    }
}

fn unreachable_config() -> WebhookConfig {
    let reserved = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = config(private_url(reserved.local_addr().unwrap()));
    drop(reserved);
    cfg
}

fn malformed_config() -> WebhookConfig {
    config(private_url(format!("127.0.0.1:{BODY_SECRET}")))
}

#[test]
fn sync_request_errors_drop_the_entire_sensitive_source_chain() {
    for (cfg, category) in [
        (unreachable_config(), "connection or TLS failure"),
        (malformed_config(), "invalid request"),
    ] {
        assert_private(
            &send_webhook(&cfg, &WebhookPayload::default()).unwrap_err(),
            category,
        );
    }
}

#[tokio::test]
async fn async_request_errors_drop_the_entire_sensitive_source_chain() {
    for (cfg, category) in [
        (unreachable_config(), "connection or TLS failure"),
        (malformed_config(), "invalid request"),
    ] {
        assert_private(
            &send_webhook_async(&cfg, &WebhookPayload::default())
                .await
                .unwrap_err(),
            category,
        );
    }
}

fn redirect_server() -> (WebhookConfig, Arc<AtomicBool>, thread::JoinHandle<usize>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let cfg = config(private_url(server.server_addr()));
    let location = cfg.url.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        let mut requests = 0;
        while !worker_stop.load(Ordering::Acquire) {
            if let Some(request) = server.recv_timeout(Duration::from_millis(20)).unwrap() {
                requests += 1;
                request
                    .respond(tiny_http::Response::empty(302).with_header(
                        tiny_http::Header::from_bytes("Location", location.as_str()).unwrap(),
                    ))
                    .unwrap();
            }
        }
        requests
    });
    (cfg, stop, worker)
}

#[test]
fn sync_redirect_failure_does_not_disclose_location() {
    let (cfg, stop, worker) = redirect_server();
    let error = send_webhook(&cfg, &WebhookPayload::default()).unwrap_err();
    stop.store(true, Ordering::Release);
    assert!(worker.join().unwrap() > 1);
    assert_private(&error, "redirect failure");
}

#[tokio::test]
async fn async_redirect_failure_does_not_disclose_location() {
    let (cfg, stop, worker) = redirect_server();
    let error = send_webhook_async(&cfg, &WebhookPayload::default())
        .await
        .unwrap_err();
    stop.store(true, Ordering::Release);
    assert!(worker.join().unwrap() > 1);
    assert_private(&error, "redirect failure");
}

fn stalled_server() -> (WebhookConfig, thread::JoinHandle<()>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let cfg = config(private_url(server.server_addr()));
    let worker = thread::spawn(move || {
        let request = server
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .expect("webhook request");
        thread::sleep(Duration::from_millis(1200));
        drop(request);
    });
    (cfg, worker)
}

#[test]
fn sync_timeout_keeps_classification_without_url() {
    let (cfg, worker) = stalled_server();
    let error = send_webhook(&cfg, &WebhookPayload::default()).unwrap_err();
    worker.join().unwrap();
    assert_private(&error, "timeout");
}

#[tokio::test]
async fn async_timeout_keeps_classification_without_url() {
    let (cfg, worker) = stalled_server();
    let error = send_webhook_async(&cfg, &WebhookPayload::default())
        .await
        .unwrap_err();
    worker.join().unwrap();
    assert_private(&error, "timeout");
}

fn invalid_response_server(tls: bool) -> (WebhookConfig, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut url = private_url(listener.local_addr().unwrap());
    if tls {
        url = url.replacen("http://", "https://", 1);
    }
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "fixture received no connection");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        // A plaintext response to a TLS handshake, or an invalid HTTP status
        // line, produces a genuine nested transport/parser error.
        let _ = stream.write_all(format!("{BODY_SECRET}\r\n\r\n").as_bytes());
    });
    (config(url), worker)
}

#[test]
fn sync_tls_and_response_parser_failures_drop_sensitive_causes() {
    for tls in [true, false] {
        let (cfg, worker) = invalid_response_server(tls);
        let error = send_webhook(&cfg, &WebhookPayload::default()).unwrap_err();
        worker.join().unwrap();
        assert_private(&error, "webhook delivery failed");
    }
}

#[tokio::test]
async fn async_tls_and_response_parser_failures_drop_sensitive_causes() {
    for tls in [true, false] {
        let (cfg, worker) = invalid_response_server(tls);
        let error = send_webhook_async(&cfg, &WebhookPayload::default())
            .await
            .unwrap_err();
        worker.join().unwrap();
        assert_private(&error, "webhook delivery failed");
    }
}
