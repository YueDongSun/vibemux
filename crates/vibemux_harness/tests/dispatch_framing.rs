//! Bounded JSONL framing and capture accounting.

use std::num::NonZeroUsize;

use proptest::prelude::*;
use vibemux_harness::dispatch::{
    DispatchError, DispatchLimits, NativeProtocol, ObservationKind, Sha256Digest,
    capture_budget::CaptureBudget, json_line_framer::JsonLineFramer,
};

const CODEX_EXEC: &str = include_str!("fixtures/dispatch/codex_exec.jsonl");
const ACP_STREAM: &str = include_str!("fixtures/dispatch/acp_stream.jsonl");

fn framer(limit: usize) -> JsonLineFramer {
    JsonLineFramer::new(NonZeroUsize::new(limit).expect("nonzero limit"))
}

fn frame_all(limit: usize, chunks: &[&[u8]]) -> Result<Vec<String>, DispatchError> {
    let mut framer = framer(limit);
    let mut frames = Vec::new();
    for chunk in chunks {
        framer.push(chunk, &mut frames)?;
    }
    frames.extend(framer.finish()?);
    Ok(frames)
}

#[test]
fn multibyte_utf8_and_crlf_split_across_chunks_reassemble() {
    let frames = frame_all(
        128,
        &[
            b"{\"text\":\"\xe4",
            b"\xbd\xa0\xe5\xa5\xbd\"}\r",
            b"\n{}",
            b"\n",
        ],
    )
    .expect("frames");
    assert_eq!(frames, ["{\"text\":\"\u{4f60}\u{597d}\"}", "{}"]);
}

#[test]
fn final_frame_without_newline_is_accepted_once() {
    let mut framer = framer(16);
    let mut frames = Vec::new();
    framer.push(b"{}\n{\"a\":1}", &mut frames).unwrap();
    assert_eq!(frames, ["{}"]);
    assert_eq!(framer.pending_bytes(), 7);
    assert_eq!(framer.finish().unwrap().as_deref(), Some("{\"a\":1}"));
    assert_eq!(framer.finish().unwrap(), None);
}

#[test]
fn empty_frames_and_invalid_utf8_fail_explicitly() {
    assert_eq!(frame_all(8, &[b"\n"]), Err(DispatchError::EmptyRecord));
    assert_eq!(frame_all(8, &[b"\r\n"]), Err(DispatchError::EmptyRecord));
    assert_eq!(frame_all(8, &[b"\xff\n"]), Err(DispatchError::InvalidUtf8));
    assert_eq!(
        frame_all(8, &[b"\xe4\xbd"]),
        Err(DispatchError::InvalidUtf8)
    );
}

#[test]
fn frame_limit_is_exact_and_rejects_before_the_newline_arrives() {
    assert_eq!(frame_all(4, &[b"1234\n"]).unwrap(), ["1234"]);
    assert_eq!(
        frame_all(4, &[b"12345\n"]),
        Err(DispatchError::FrameTooLarge)
    );
    let mut framer = framer(4);
    let mut frames = Vec::new();
    framer.push(b"12", &mut frames).unwrap();
    assert_eq!(
        framer.push(b"345", &mut frames),
        Err(DispatchError::FrameTooLarge)
    );
    assert_eq!(framer.pending_bytes(), 0);
}

#[test]
fn a_failed_framer_stays_failed() {
    let mut framer = framer(8);
    let mut frames = Vec::new();
    assert_eq!(
        framer.push(b"{}\n\n{}\n", &mut frames),
        Err(DispatchError::EmptyRecord)
    );
    assert_eq!(frames, ["{}"]);
    assert_eq!(
        framer.push(b"{}\n", &mut frames),
        Err(DispatchError::EmptyRecord)
    );
    assert_eq!(framer.finish(), Err(DispatchError::EmptyRecord));
    assert_eq!(frames, ["{}"]);
}

proptest! {
    #[test]
    fn chunk_boundaries_never_change_the_frames(cuts in prop::collection::vec(0_usize..2048, 0..24)) {
        let bytes = format!("{ACP_STREAM}{CODEX_EXEC}").into_bytes();
        let mut cuts: Vec<usize> = cuts.into_iter().map(|cut| cut % (bytes.len() + 1)).collect();
        cuts.sort_unstable();
        let mut chunks = Vec::new();
        let mut start = 0;
        for cut in cuts {
            chunks.push(&bytes[start..cut]);
            start = cut;
        }
        chunks.push(&bytes[start..]);
        let frames = frame_all(4096, &chunks).expect("fixture frames");
        let expected: Vec<&str> = ACP_STREAM.lines().chain(CODEX_EXEC.lines()).collect();
        prop_assert_eq!(frames, expected);
    }
}

#[test]
fn capture_assigns_sequences_counts_kinds_and_hashes_the_canonical_transcript() {
    let mut budget = CaptureBudget::new(NativeProtocol::CodexExec, &DispatchLimits::default())
        .expect("default limits");
    let mut sequences = Vec::new();
    for line in CODEX_EXEC.lines() {
        let (record, _) = budget.accept(line.to_string()).expect("record");
        sequences.push(record.sequence());
    }
    assert_eq!(sequences, [1, 2, 3, 4, 5, 6, 7]);
    let summary = budget.summary();
    assert_eq!(summary.record_count, 7);
    assert_eq!(
        summary.record_bytes,
        CODEX_EXEC.lines().map(str::len).sum::<usize>() as u64
    );
    assert_eq!(summary.kinds.started, 2);
    assert_eq!(summary.kinds.text, 2);
    assert_eq!(summary.kinds.tool, 1);
    assert_eq!(summary.kinds.opaque, 1);
    assert_eq!(summary.kinds.completed, 1);
    assert_eq!(summary.kinds.total(), 7);
    let canonical: String = CODEX_EXEC.lines().flat_map(|line| [line, "\n"]).collect();
    assert_eq!(
        summary.transcript_sha256,
        Sha256Digest::of(canonical.as_bytes())
    );
}

#[test]
fn capture_limits_fail_closed_and_rejected_records_are_not_counted() {
    let limits = DispatchLimits {
        record_count: 1,
        ..DispatchLimits::default()
    };
    let mut budget = CaptureBudget::new(NativeProtocol::Acp, &limits).unwrap();
    assert_eq!(
        budget.accept("[]".to_string()).unwrap_err(),
        DispatchError::InvalidRecord
    );
    assert_eq!(budget.summary().record_count, 0);
    let (record, _) = budget.accept("{}".to_string()).unwrap();
    assert_eq!(record.kind(), ObservationKind::Opaque);
    assert_eq!(
        budget.accept("{}".to_string()).unwrap_err(),
        DispatchError::CaptureTooLarge
    );

    let limits = DispatchLimits {
        frame_bytes: 64,
        capture_bytes: 64,
        ..DispatchLimits::default()
    };
    let mut budget = CaptureBudget::new(NativeProtocol::Acp, &limits).unwrap();
    let filler = format!("{{\"pad\":\"{}\"}}", "x".repeat(50));
    assert_eq!(filler.len(), 60);
    budget.accept(filler).unwrap();
    assert_eq!(
        budget.accept("{\"a\":1}".to_string()).unwrap_err(),
        DispatchError::CaptureTooLarge
    );
    assert_eq!(budget.summary().record_bytes, 60);

    let invalid = DispatchLimits {
        record_count: 0,
        ..DispatchLimits::default()
    };
    assert_eq!(
        CaptureBudget::new(NativeProtocol::Acp, &invalid).unwrap_err(),
        DispatchError::ConfigInvalid
    );
}
