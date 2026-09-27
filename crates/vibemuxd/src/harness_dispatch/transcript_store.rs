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

use uuid::Uuid;
use vibemux_harness::dispatch::{DispatchError, ObservedRecord};

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
        let (transcript, complete) = match state.live.get(&request_id) {
            Some(transcript) => (transcript, false),
            None => state
                .finished
                .iter()
                .find(|(id, _)| *id == request_id)
                .map(|(_, transcript)| (transcript, true))
                .ok_or(DispatchError::OutputUnavailable)?,
        };
        let records = transcript
            .records
            .iter()
            .filter(|record| record.sequence() > after_sequence)
            .take(max_records)
            .cloned()
            .collect();
        Ok(TranscriptPage { records, complete })
    }

    fn lock(&self) -> MutexGuard<'_, TranscriptState> {
        // Every update leaves the state consistent before it can panic, so
        // a poisoned lock still guards valid data.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TranscriptState {
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
        let page = store.records(request_id, 1, 1).expect("live page");
        assert!(!page.complete);
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].sequence(), 2);
        store.finish(request_id);
        let page = store.records(request_id, 0, 8).expect("finished page");
        assert!(page.complete);
        assert_eq!(page.records.len(), 3);
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
