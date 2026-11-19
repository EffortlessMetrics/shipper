use std::io::{self, Cursor, Write};
use std::time::Duration;

use super::*;
use crate::ops::process::{run_command_with_publish_capture, run_command_with_timeout};

#[test]
fn complete_capture_matches_tail_semantics_across_chunk_boundaries() {
    for input in [
        "",
        "a",
        "a\n",
        "a\r\nb\r\n",
        "a\rb\n\n",
        "\n\n",
        "終わり🙂\r\nnext",
        "Authorization: Bearer sentinel\nToken=secret\nlast\n",
    ] {
        for lines in [0, 1, 2, 100, usize::MAX] {
            for boundary in 0..=input.len() {
                let mut tail = TailCapture::new(lines);
                tail.push(&input.as_bytes()[..boundary]);
                tail.push(&input.as_bytes()[boundary..]);
                let (text, loss) = tail.finish();
                assert_eq!(loss, None, "{input:?} {lines} {boundary}");
                assert_eq!(
                    shipper_output_sanitizer::tail_lines(&text, lines),
                    shipper_output_sanitizer::tail_lines(input, lines),
                    "{input:?} {lines} {boundary}"
                );
            }
        }
    }
}

#[test]
fn limits_bound_partial_lines_payload_entries_and_allocated_capacity() {
    let mut exact_line = TailCapture::new(1);
    exact_line.push(&vec![b'x'; LINE_BYTES]);
    assert!(exact_line.partial.capacity() <= LINE_BYTES);
    assert_eq!(exact_line.finish().0.len(), LINE_BYTES);

    let mut oversized = TailCapture::new(50);
    oversized.push(b"Authorization: Bearer ");
    for _ in 0..1024 {
        oversized.push(&[b'x'; READ_BYTES]);
        assert!(oversized.partial.capacity() <= LINE_BYTES);
        assert!(oversized.retained_bytes <= PUBLISH_TAIL_BYTES);
    }
    oversized.push(b"secret-suffix\r\n continuation-secret\npermission denied\n");
    assert_eq!(
        oversized.finish(),
        (String::new(), Some(CaptureLoss::LineBytes))
    );

    let mut bytes = TailCapture::new(usize::MAX);
    for _ in 0..4 {
        bytes.push(&vec![b'x'; LINE_BYTES - 1]);
        bytes.push(b"\n");
    }
    assert_eq!(bytes.retained_bytes, PUBLISH_TAIL_BYTES);
    bytes.push(b"x\n");
    assert_eq!(
        bytes.finish(),
        (String::new(), Some(CaptureLoss::TailBytes))
    );

    let mut entries = TailCapture::new(usize::MAX);
    entries.push(&vec![b'\n'; TAIL_ENTRIES]);
    assert_eq!(entries.lines.len(), TAIL_ENTRIES);
    assert!(entries.lines.capacity() <= TAIL_ENTRIES);
    entries.push(b"\n");
    assert_eq!(
        entries.finish(),
        (String::new(), Some(CaptureLoss::TailEntries))
    );
}

#[test]
fn requested_tail_eviction_and_zero_lines_are_not_capture_loss() {
    let mut tail = TailCapture::new(1);
    let mut empty = TailCapture::new(0);
    let line = vec![b'x'; LINE_BYTES];
    for _ in 0..100 {
        tail.push(&line);
        tail.push(b"\n");
        empty.push(&line);
        assert!(tail.lines.len() <= 1);
        assert!(empty.partial.is_empty());
        assert!(empty.lines.is_empty());
    }
    tail.push(b"final");
    assert_eq!(tail.finish(), ("final".into(), None));
    assert_eq!(empty.finish(), (String::new(), None));
}

#[test]
fn invalid_utf8_is_decoded_only_after_line_framing_and_stays_bounded() {
    let input = b"first\n\xf0\x9f\x99\x82\xff\r\nlast";
    let mut tail = TailCapture::new(3);
    for byte in input {
        tail.push(&[*byte]);
    }
    assert_eq!(tail.finish(), ("first\n🙂�\nlast".into(), None));
    let mut expansion = TailCapture::new(10);
    for _ in 0..2 {
        expansion.push(&vec![0xff; LINE_BYTES]);
        expansion.push(b"\n");
    }
    assert_eq!(
        expansion.finish(),
        (String::new(), Some(CaptureLoss::TailBytes))
    );
}

struct FailingReader(u8);

impl Read for FailingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0 += 1;
        match self.0 {
            1 => Err(io::Error::from(ErrorKind::Interrupted)),
            2 => {
                buffer[..8].copy_from_slice(b"partial\n");
                Ok(8)
            }
            _ => Err(io::Error::other("fixture read failure")),
        }
    }
}

#[test]
fn read_failure_is_typed_even_when_empty_output_was_requested() {
    for lines in [0, 100] {
        let capture = CaptureBuffer::read(Some(FailingReader(0)), CaptureMode::PublishTail(lines));
        assert_eq!(capture.finish(), (String::new(), Some(CaptureLoss::Read)));
    }
    let capture = CaptureBuffer::read(Some(Cursor::new(b"ok\n")), CaptureMode::PublishTail(5));
    assert_eq!(capture.finish(), ("ok\n".into(), None));
}

struct InvalidCountReader {
    prefix: Cursor<&'static [u8]>,
    count: usize,
}

impl Read for InvalidCountReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.prefix.read(buffer)?;
        Ok(if read == 0 { self.count } else { read })
    }
}

#[test]
fn oversized_read_count_discards_partial_capture_with_typed_loss() {
    for prefix in [b"".as_slice(), b"first\npartial".as_slice()] {
        for count in [READ_BYTES + 1, usize::MAX] {
            for lines in [0, 1, 100] {
                let reader = InvalidCountReader {
                    prefix: Cursor::new(prefix),
                    count,
                };
                let capture = CaptureBuffer::read(Some(reader), CaptureMode::PublishTail(lines));
                assert_eq!(
                    capture.finish(),
                    (String::new(), Some(CaptureLoss::Read)),
                    "prefix={prefix:?} count={count} lines={lines}"
                );
            }
        }
    }
}

#[test]
fn valid_reads_crossing_the_buffer_boundary_preserve_complete_output() {
    for length in [READ_BYTES - 1, READ_BYTES, READ_BYTES + 1] {
        let input = vec![b'x'; length];
        let capture = CaptureBuffer::read(Some(Cursor::new(&input)), CaptureMode::PublishTail(1));
        assert_eq!(capture.finish(), ("x".repeat(length), None));
    }
}

const STREAM_CHILD: &str = "ops::process::capture::tests::capture_stream_child";

#[test]
#[serial_test::serial]
#[ignore = "actual stream producer invoked by bounded/full capture tests"]
fn capture_stream_child() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(45));
        std::process::exit(99);
    });
    let mode = std::fs::read_to_string("capture-mode").expect("mode");
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    if mode == "json" {
        write!(out, "CAPTURE_JSON={{\"payload\":\"").unwrap();
        for _ in 0..128 {
            out.write_all(&[b'j'; READ_BYTES]).unwrap();
        }
        writeln!(out, "\",\"sentinel\":\"complete\"}}").unwrap();
    } else {
        let mut chunk = [b'x'; READ_BYTES];
        if mode == "lines" {
            chunk[READ_BYTES - 1] = b'\n';
        }
        // 64 MiB per stream, generated from a fixed-size buffer. The consumer
        // must retain a bounded tail while it drains this actual subprocess.
        for _ in 0..8192 {
            out.write_all(&chunk).unwrap();
            err.write_all(&chunk).unwrap();
        }
        writeln!(out, "\nstdout final").unwrap();
        writeln!(err, "\nstderr final").unwrap();
    }
    out.flush().unwrap();
    err.flush().unwrap();
    std::process::exit(0);
}

#[test]
#[serial_test::serial]
fn generic_complete_output_preserves_large_structured_stdout() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("capture-mode"), "json").unwrap();
    let exe = std::env::current_exe().unwrap();
    for timeout in [None, Some(Duration::from_secs(10))] {
        let result = run_command_with_timeout(
            exe.to_str().unwrap(),
            &[
                "--exact",
                STREAM_CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ],
            directory.path(),
            timeout,
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(!result.timed_out);
        let json = result
            .stdout
            .lines()
            .find_map(|line| line.split_once("CAPTURE_JSON=").map(|(_, rest)| rest))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(value["payload"].as_str().unwrap().len(), 128 * READ_BYTES);
        assert_eq!(value["sentinel"], "complete");
    }
}

#[cfg(target_os = "linux")]
fn peak_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
#[ignore = "fresh-process peak-memory witness invoked by parent test"]
fn bounded_capture_memory_child() {
    let exe = std::env::current_exe().unwrap();
    let baseline = peak_kib();
    let result = run_command_with_publish_capture(
        exe.to_str().unwrap(),
        &["--exact", STREAM_CHILD, "--ignored", "--nocapture"],
        &std::env::current_dir().unwrap(),
        Some(Duration::from_secs(30)),
        16,
    )
    .unwrap();
    let growth = peak_kib().saturating_sub(baseline);
    // Check actual process high-water memory before any output oracle. A
    // read_to_end-then-tail implementation must fail this independent bound.
    assert!(
        growth < 32 * 1024,
        "capture peak grew {growth} KiB; baseline {baseline} KiB"
    );
    assert_eq!(result.output.exit_code, 0);
    assert!(!result.output.timed_out);
    let mode = std::fs::read_to_string("capture-mode").unwrap();
    if mode == "lines" {
        assert!(result.capture.is_complete());
        assert!(result.output.stdout.ends_with("stdout final"));
        assert!(result.output.stderr.ends_with("stderr final"));
    } else {
        assert_eq!(result.capture.stdout, Some(CaptureLoss::LineBytes));
        assert_eq!(result.capture.stderr, Some(CaptureLoss::LineBytes));
        assert!(result.output.stdout.is_empty() && result.output.stderr.is_empty());
    }
    println!("bounded production capture peak growth: {growth} KiB ({mode})");
}

#[cfg(target_os = "linux")]
#[test]
#[serial_test::serial]
fn large_streams_have_bounded_peak_memory_with_and_without_newlines() {
    let directory = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    for mode in ["lines", "unterminated"] {
        std::fs::write(directory.path().join("capture-mode"), mode).unwrap();
        let output = std::process::Command::new(&exe)
            .args([
                "--exact",
                "ops::process::capture::tests::bounded_capture_memory_child",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
