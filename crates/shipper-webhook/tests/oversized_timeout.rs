//! Keep the public blocking caller's oversized-timeout regression finite.

use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use shipper_webhook::{WebhookConfig, WebhookPayload, send_webhook};

const PROBE_ENV: &str = "SHIPPER_REQWEST_OVERSIZED_TIMEOUT_PROBE";

struct RunningProbe(Option<Child>);

impl Drop for RunningProbe {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn blocking_webhook_delivers_with_an_unrepresentable_timeout() {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args([
            "--exact",
            "oversized_timeout_delivery_probe",
            "--ignored",
            "--nocapture",
        ])
        .env(PROBE_ENV, "1")
        .env("RUST_BACKTRACE", "0")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Only the isolated child changes its environment; other tests keep their
    // proxy settings. This witness must never leave the loopback interface.
    for variable in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(variable);
    }
    let mut running = RunningProbe(Some(command.spawn().expect("start timeout probe")));
    let deadline = Instant::now() + Duration::from_secs(10);
    let timed_out = loop {
        if running.0.as_mut().unwrap().try_wait().unwrap().is_some() {
            break false;
        }
        if Instant::now() >= deadline {
            running.0.as_mut().unwrap().kill().unwrap();
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = running.0.take().unwrap().wait_with_output().unwrap();
    assert!(
        !timed_out && output.status.success(),
        "oversized-timeout delivery failed (watchdog expired: {timed_out}, status: {}):\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
#[ignore = "finite loopback fixture invoked only by the parent timeout witness"]
fn oversized_timeout_delivery_probe() {
    assert_eq!(std::env::var(PROBE_ENV).as_deref(), Ok("1"));
    let server = tiny_http::Server::http("127.0.0.1:0").expect("loopback server");
    let config: WebhookConfig = serde_json::from_value(serde_json::json!({
        "url": format!("http://{}/oversized-timeout", server.server_addr()),
        // This value is also representable in Shipper's TOML configuration.
        "timeout_secs": i64::MAX,
    }))
    .expect("accepted public webhook configuration");
    assert!(
        Instant::now()
            .checked_add(Duration::from_secs(config.timeout_secs))
            .is_none(),
        "the regression input must overflow this platform's Instant"
    );
    let server_worker = thread::spawn(move || {
        let mut request = server
            .recv_timeout(Duration::from_secs(3))
            .expect("receive local webhook")
            .expect("webhook request before fixture deadline");
        assert_eq!(request.method(), &tiny_http::Method::Post);
        assert_eq!(request.url(), "/oversized-timeout");
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        let payload: WebhookPayload = serde_json::from_str(&body).unwrap();
        assert_eq!(payload.message, "finite oversized-timeout witness");
        assert!(payload.success);
        request.respond(tiny_http::Response::empty(204)).unwrap();
    });
    let delivery = send_webhook(
        &config,
        &WebhookPayload {
            message: "finite oversized-timeout witness".to_string(),
            success: true,
            ..Default::default()
        },
    );
    server_worker.join().expect("loopback server completed");
    delivery.expect("immediate loopback webhook delivery without timeout overflow");
}
