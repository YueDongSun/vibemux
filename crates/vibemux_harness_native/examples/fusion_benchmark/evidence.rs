//! Append-only, content-free experiment evidence. This is not the core event log.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use vibemux_harness_native::Error;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn hash_file(path: &Path) -> Result<String, Error> {
    let mut file = File::open(path).map_err(|_| Error::new("fusion_fingerprint_failed"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    // Streaming and bounded even if the configured executable is unexpectedly huge.
    let mut remaining = 1024_u64 * 1024 * 1024;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| Error::new("fusion_fingerprint_failed"))?;
        if count == 0 {
            break;
        }
        remaining = remaining
            .checked_sub(count as u64)
            .ok_or_else(|| Error::new("fusion_executable_too_large"))?;
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn now() -> String {
    // RFC3339 formatting of the UTC clock has no operator-controlled fields.
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unavailable".to_string())
}

pub struct Evidence {
    file: Arc<Mutex<File>>,
}

impl Evidence {
    pub async fn create(path: PathBuf) -> Result<Self, Error> {
        tokio::task::spawn_blocking(move || {
            // create_dir fails on every existing path, including links. Never
            // append to another experiment or overwrite its report.
            std::fs::create_dir(&path)
                .map_err(|_| Error::new("fusion_output_must_be_new_directory"))?;
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path.join("events.jsonl"))
                .map_err(|_| Error::new("fusion_evidence_create_failed"))?;
            Ok(Self {
                file: Arc::new(Mutex::new(file)),
            })
        })
        .await
        .map_err(|_| Error::new("fusion_evidence_worker_failed"))?
    }

    pub async fn record(&self, record: Value) -> Result<(), Error> {
        let mut bytes =
            serde_json::to_vec(&record).map_err(|_| Error::new("fusion_evidence_encode_failed"))?;
        bytes.push(b'\n');
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || {
            let mut file = file
                .lock()
                .map_err(|_| Error::new("fusion_evidence_lock_failed"))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_data())
                .map_err(|_| Error::new("fusion_evidence_write_failed"))
        })
        .await
        .map_err(|_| Error::new("fusion_evidence_worker_failed"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn existing_evidence_is_never_reopened_or_truncated() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("experiment");
        let evidence = Evidence::create(output.clone()).await.unwrap();
        evidence
            .record(serde_json::json!({"record_type":"first"}))
            .await
            .unwrap();
        assert!(Evidence::create(output.clone()).await.is_err());
        evidence
            .record(serde_json::json!({"record_type":"second"}))
            .await
            .unwrap();
        let text = std::fs::read_to_string(output.join("events.jsonl")).unwrap();
        let records: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["record_type"], "first");
        assert_eq!(records[1]["record_type"], "second");
    }
}
