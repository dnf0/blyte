//! Timing of a streamed Gemini call: when its first data arrived and the longest silence in it.
//! The reader sleeps before each step, so lower bounds hold; upper bounds are kept loose.

use std::collections::VecDeque;
use std::io::{self, BufReader, Read};
use std::time::{Duration, Instant};

use blyte_provider::gemini::Gemini;
use blyte_provider::{ProviderError, StreamTiming};

/// One read from the scripted stream: sleep `pause_ms`, then return `bytes` or fail with `kind`.
struct Step {
    pause_ms: u64,
    result: Result<Vec<u8>, io::ErrorKind>,
}

struct Scripted {
    steps: VecDeque<Step>,
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(step) = self.steps.pop_front() else {
            return Ok(0);
        };
        std::thread::sleep(Duration::from_millis(step.pause_ms));
        match step.result {
            Ok(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                Ok(n)
            }
            Err(kind) => Err(io::Error::new(kind, "scripted failure")),
        }
    }
}

fn data(text: &str) -> Step {
    Step {
        pause_ms: 0,
        result: Ok(format!(
            "data: {{\"candidates\":[{{\"content\":{{\"role\":\"model\",\"parts\":[{{\"text\":\"{text}\"}}]}}}}]}}\n\n"
        )
        .into_bytes()),
    }
}

fn after(pause_ms: u64, mut step: Step) -> Step {
    step.pause_ms = pause_ms;
    step
}

fn timed(
    steps: Vec<Step>,
) -> (
    Result<blyte_provider::Response, ProviderError>,
    StreamTiming,
) {
    let reader = BufReader::new(Scripted {
        steps: steps.into(),
    });
    let mut timing = StreamTiming::default();
    let result =
        Gemini::parse_stream_timed(reader, Duration::from_secs(60), Instant::now(), &mut timing);
    (result, timing)
}

#[test]
fn timing_of_a_stream_with_a_pause() {
    let before = Instant::now();
    let (result, timing) = timed(vec![after(40, data("a")), data("b"), after(120, data("c"))]);
    let elapsed_ms = u64::try_from(before.elapsed().as_millis()).unwrap_or(u64::MAX);
    result.unwrap();
    assert_eq!(timing.data_events, 3);
    let first = timing.first_data_ms.expect("first data was seen");
    assert!(
        (40..=elapsed_ms).contains(&first),
        "first_data_ms {first} (elapsed {elapsed_ms})"
    );
    assert!(
        (120..=elapsed_ms).contains(&timing.max_gap_ms),
        "max_gap_ms {} (elapsed {elapsed_ms})",
        timing.max_gap_ms
    );
}

#[test]
fn timing_of_an_empty_stream() {
    let (_result, timing) = timed(vec![Step {
        pause_ms: 50,
        result: Ok(Vec::new()),
    }]);
    assert_eq!(timing.first_data_ms, None);
    assert_eq!(timing.data_events, 0);
    assert!(timing.max_gap_ms >= 50, "max_gap_ms {}", timing.max_gap_ms);
}

#[test]
fn timing_is_filled_when_the_read_fails() {
    let (result, timing) = timed(vec![
        data("a"),
        Step {
            pause_ms: 60,
            result: Err(io::ErrorKind::TimedOut),
        },
    ]);
    assert!(result.is_err());
    assert!(timing.first_data_ms.is_some());
    assert_eq!(timing.data_events, 1);
    assert!(timing.max_gap_ms >= 60, "max_gap_ms {}", timing.max_gap_ms);
}

#[test]
fn comment_and_event_lines_are_not_data() {
    let bytes = concat!(
        ": ping\n",
        "event: x\n",
        "\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"a\"}]}}]}\n\n",
    );
    let (result, timing) = timed(vec![Step {
        pause_ms: 0,
        result: Ok(bytes.as_bytes().to_vec()),
    }]);
    result.unwrap();
    assert_eq!(timing.data_events, 1);
}

#[test]
fn parse_stream_matches_parse_stream_timed() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hel\"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lo\"}]},\"finishReason\":\"STOP\"}]}\n\n",
    );
    let plain = Gemini::parse_stream(
        std::io::Cursor::new(sse.as_bytes()),
        Duration::from_secs(60),
        Instant::now(),
    )
    .unwrap();
    let mut timing = StreamTiming::default();
    let timed = Gemini::parse_stream_timed(
        std::io::Cursor::new(sse.as_bytes()),
        Duration::from_secs(60),
        Instant::now(),
        &mut timing,
    )
    .unwrap();
    assert_eq!(plain, timed);
    assert_eq!(timing.data_events, 2);
}
