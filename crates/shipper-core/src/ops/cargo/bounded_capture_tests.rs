use super::*;

fn captured(
    stdout: &str,
    stderr: &str,
    capture: process::CaptureStatus,
) -> process::CapturedCommandOutput {
    process::CapturedCommandOutput {
        cleanup_failure: None,
        interrupted: false,
        interruption_lease: None,
        output: process::CommandOutput {
            exit_code: 42,
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
            duration: Duration::ZERO,
        },
        capture,
    }
}

#[test]
fn either_stream_loss_suppresses_both_streams_without_changing_result() {
    for stdout_lost in [false, true] {
        for lines in [0, 10] {
            let status = process::CaptureStatus {
                stdout: stdout_lost.then_some(process::CaptureLoss::LineBytes),
                stderr: (!stdout_lost).then_some(process::CaptureLoss::Read),
            };
            let result = finish_publish_capture(
                captured("credential-suffix", "permission denied", status),
                lines,
                Duration::from_secs(1),
            );
            assert_eq!(result.capture, status);
            assert_eq!(result.output.exit_code, 42);
            assert!(!result.output.timed_out);
            assert_eq!(result.output.duration, Duration::from_secs(1));
            assert!(result.output.stdout_tail.is_empty());
            assert_eq!(
                result.output.stderr_tail,
                if lines == 0 {
                    ""
                } else {
                    PUBLISH_CAPTURE_OMITTED
                }
            );
        }
    }
}

#[test]
fn sanitizer_expansion_cannot_exceed_persisted_payload_limit() {
    let line = "Token=x ".repeat(6000);
    let input = format!("{line}\n{line}\n{line}\n{line}\n");
    assert!(input.len() < process::PUBLISH_TAIL_BYTES);
    assert!(tail_lines(&input, 10).len() > process::PUBLISH_TAIL_BYTES);
    let result = finish_publish_capture(
        captured(
            &input,
            "otherwise complete",
            process::CaptureStatus::default(),
        ),
        10,
        Duration::ZERO,
    );
    assert_eq!(result.capture.stdout, Some(process::CaptureLoss::TailBytes));
    assert!(result.output.stdout_tail.is_empty());
    assert_eq!(result.output.stderr_tail, PUBLISH_CAPTURE_OMITTED);
}

#[test]
fn complete_tails_preserve_existing_redaction_and_typed_timeout() {
    let mut source = captured(
        "Authorization: Bearer secret\nlast",
        "Token=secret\n",
        process::CaptureStatus::default(),
    );
    source.output.exit_code = -1;
    source.output.timed_out = true;
    let result = finish_publish_capture(source, 10, Duration::ZERO);
    assert!(result.capture.is_complete());
    assert!(result.output.timed_out);
    assert_eq!(result.output.exit_code, -1);
    assert_eq!(result.output.stdout_tail, "Authorization: [REDACTED]\nlast");
    assert_eq!(result.output.stderr_tail, "Token=[REDACTED]\n");
}
