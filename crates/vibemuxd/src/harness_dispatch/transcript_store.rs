//! In-memory transcripts of captured vendor records (ADR 029 §5).
//!
//! Records are byte-exact vendor JSON and may hold anything the vendor
//! printed, so they never reach the canonical store, an event, or a log;
//! they leave the daemon only through the explicit output read. A live
//! attempt's capture is already bounded by its `CaptureBudget`. Finished
//! transcripts are retained up to [`MAX_RETAINED_TRANSCRIPTS`] and
//! [`MAX_RETAINED_TRANSCRIPT_BYTES`], oldest evicted first, and are lost on
//! restart, after which reads fail with `harness_dispatch_output_unavailable`.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, MutexGuard, PoisonError},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vibemux_harness::dispatch::{DispatchError, ObservedRecord};

use super::output_page::{DispatchOutputPage, OutputCursor, OutputPageLimits, build_output_page};

pub const MAX_RETAINED_TRANSCRIPTS: usize = 4;
pub const MAX_RETAINED_TRANSCRIPT_BYTES: usize = 64 * 1024 * 1024;

/// One read of a transcript.
#[derive(Debug)]
pub struct TranscriptPage {
    /// Records after the requested sequence, in capture order.
    pub records: Vec<ObservedRecord>,
    /// The attempt finished capturing; no record will be added.
    pub complete: bool,
}

/// Content-free size of a transcript still capturing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveCapture {
    pub record_count: u64,
    pub record_bytes: u64,
}

#[derive(Debug, Default)]
struct Transcript {
    records: Vec<ObservedRecord>,
    bytes: usize,
}

#[derive(Debug, Default)]
struct TranscriptState {
    live: HashMap<Uuid, Transcript>,
    finished: VecDeque<(Uuid, Transcript)>,
    finished_bytes: usize,
}

#[derive(Debug, Default)]
pub struct TranscriptStore {
    state: Mutex<TranscriptState>,
}

impl TranscriptStore {
    /// Starts an empty live transcript, replacing a retained one with the
    /// same id.
    pub fn begin(&self, request_id: Uuid) {
        let mut state = self.lock();
        state.remove_finished(request_id);
        state.live.insert(request_id, Transcript::default());
    }

    pub fn append(&self, request_id: Uuid, record: ObservedRecord) {
        if let Some(transcript) = self.lock().live.get_mut(&request_id) {
            transcript.bytes += record.byte_count();
            transcript.records.push(record);
        }
    }

    /// Retains the live transcript as finished and evicts the oldest ones
    /// beyond the bounds.
    pub fn finish(&self, request_id: Uuid) {
        let mut state = self.lock();
        let Some(transcript) = state.live.remove(&request_id) else {
            return;
        };
        state.finished_bytes += transcript.bytes;
        state.finished.push_back((request_id, transcript));
        while state.finished.len() > MAX_RETAINED_TRANSCRIPTS
            || state.finished_bytes > MAX_RETAINED_TRANSCRIPT_BYTES
        {
            let Some((_, evicted)) = state.finished.pop_front() else {
                break;
            };
            state.finished_bytes -= evicted.bytes;
        }
    }

    /// Up to `max_records` records with a sequence above `after_sequence`.
    pub fn records(
        &self,
        request_id: Uuid,
        after_sequence: u64,
        max_records: usize,
    ) -> Result<TranscriptPage, DispatchError> {
        let state = self.lock();
        let (transcript, complete) = state.find(request_id)?;
        let records = transcript
            .records
            .iter()
            .filter(|record| record.sequence() > after_sequence)
            .take(max_records)
            .cloned()
            .collect();
        Ok(TranscriptPage { records, complete })
    }

    /// The page at `cursor`, built under the lock so only the page's bytes
    /// are copied.
    pub fn output_page(
        &self,
        request_id: Uuid,
        cursor: OutputCursor,
        limits: OutputPageLimits,
    ) -> Result<DispatchOutputPage, DispatchError> {
        let state = self.lock();
        let (transcript, complete) = state.find(request_id)?;
        build_output_page(&transcript.records, complete, cursor, limits)
    }

    /// Size of the transcript an attempt is still capturing.
    pub fn live_capture(&self, request_id: Uuid) -> Option<LiveCapture> {
        self.lock()
            .live
            .get(&request_id)
            .map(|transcript| LiveCapture {
                record_count: transcript.records.len() as u64,
                record_bytes: transcript.bytes as u64,
            })
    }

    fn lock(&self) -> MutexGuard<'_, TranscriptState> {
        // Every update leaves the state consistent before it can panic, so
        // a poisoned lock still guards valid data.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TranscriptState {
    /// The transcript and whether it is finished.
    fn find(&self, request_id: Uuid) -> Result<(&Transcript, bool), DispatchError> {
        if let Some(transcript) = self.live.get(&request_id) {
            return Ok((transcript, false));
        }
        self.finished
            .iter()
            .find(|(id, _)| *id == request_id)
            .map(|(_, transcript)| (transcript, true))
            .ok_or(DispatchError::OutputUnavailable)
    }

    fn remove_finished(&mut self, request_id: Uuid) {
        if let Some(index) = self.finished.iter().position(|(id, _)| *id == request_id) {
            if let Some((_, removed)) = self.finished.remove(index) {
                self.finished_bytes -= removed.bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use vibemux_harness::dispatch::NativeProtocol;

    use super::*;

    fn record(sequence: u64, text: &str) -> ObservedRecord {
        let raw = serde_json::json!({"type": "item.updated", "text": text}).to_string();
        ObservedRecord::parse(NativeProtocol::CodexExec, sequence, raw)
            .expect("record")
            .0
    }

    #[test]
    fn a_live_transcript_pages_by_sequence_and_completes_on_finish() {
        let store = TranscriptStore::default();
        let request_id = Uuid::new_v4();
        assert_eq!(
            store.records(request_id, 0, 8).unwrap_err(),
            DispatchError::OutputUnavailable
        );
        store.begin(request_id);
        for sequence in 1..=3 {
            store.append(request_id, record(sequence, "chunk"));
        }
        assert_eq!(
            store.live_capture(request_id),
            Some(LiveCapture {
                record_count: 3,
                record_bytes: 3 * record(1, "chunk").byte_count() as u64,
            })
        );
        let page = store.records(request_id, 1, 1).expect("live page");
        assert!(!page.complete);
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].sequence(), 2);
        store.finish(request_id);
        assert_eq!(store.live_capture(request_id), None);
        let page = store.records(request_id, 0, 8).expect("finished page");
        assert!(page.complete);
        assert_eq!(page.records.len(), 3);
        let output = store
            .output_page(
                request_id,
                OutputCursor::default(),
                OutputPageLimits::within(60 * 1024),
            )
            .expect("output page");
        assert!(output.complete);
        assert_eq!(output.fragments.len(), 3);
    }

    #[test]
    fn the_oldest_finished_transcripts_are_evicted_beyond_the_count() {
        let store = TranscriptStore::default();
        let ids: Vec<Uuid> = (0..=MAX_RETAINED_TRANSCRIPTS)
            .map(|_| Uuid::new_v4())
            .collect();
        for id in &ids {
            store.begin(*id);
            store.append(*id, record(1, "chunk"));
            store.finish(*id);
        }
        assert_eq!(
            store.records(ids[0], 0, 8).unwrap_err(),
            DispatchError::OutputUnavailable
        );
        for id in &ids[1..] {
            assert!(store.records(*id, 0, 8).is_ok());
        }
    }

    #[test]
    fn the_oldest_finished_transcripts_are_evicted_beyond_the_byte_bound() {
        let store = TranscriptStore::default();
        let large = "x".repeat(MAX_RETAINED_TRANSCRIPT_BYTES / 2);
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();
        for id in &ids {
            store.begin(*id);
            store.append(*id, record(1, &large));
            store.finish(*id);
        }
        assert_eq!(
            store.records(ids[0], 0, 1).unwrap_err(),
            DispatchError::OutputUnavailable
        );
        assert!(store.records(ids[2], 0, 1).is_ok());
    }
}
