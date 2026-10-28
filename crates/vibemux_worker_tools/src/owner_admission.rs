//! Pure owner validation for a schema-1 file-only broker publication.
//!
//! The owner must supply the exact admitted bootstrap and read a bounded, complete
//! publication only after its process owner has settled. These checks confer no
//! lease authority and do not authenticate the pathless operation audit claims.

use std::collections::BTreeMap;

use crate::{
    contract::{
        Bootstrap, Candidate, MAX_FILES, MAX_OPERATIONS, MAX_TOTAL_BYTES, OperationReceipt,
        SCHEMA_VERSION, TextFile, ToolError,
    },
    workspace::{forbidden, validate_bootstrap, validate_content, validate_names, validate_path},
};
use vibemux_harness::dispatch::writable_profile::WritableGrant;

pub use crate::workspace::snapshot_digest;

// Only pre-mutation refusals emitted by schema-1 file operations. Quarantine,
// changed identity and unknown mutation cannot accompany an exportable candidate.
const REFUSAL_CODES: [ToolError; 10] = [
    ToolError::InvalidPath,
    ToolError::Denied,
    ToolError::OutsideScope,
    ToolError::ProtectedPath,
    ToolError::ContentLimit,
    ToolError::AlreadyExists,
    ToolError::Missing,
    ToolError::StaleRevision,
    ToolError::ResourceLimit,
    ToolError::Io,
];

/// Changes checked against one exact admitted snapshot. Construction stays here;
/// applying them, observing process settlement and fencing a lease belong to the owner.
pub struct ValidatedCandidate {
    changed_files: Vec<TextFile>,
}

impl ValidatedCandidate {
    pub fn into_changed_files(self) -> Vec<TextFile> {
        self.changed_files
    }
}

#[derive(Default)]
struct MutationCounts {
    creates: usize,
    edits: usize,
}

/// Checks an owner publication without reading or changing any filesystem state.
///
/// No-op edits and edits restoring the original bytes remain changes, matching
/// WorkerTools' exported changed set. Receipts can establish necessary consistency
/// only: they contain no paths and cannot prove which operation changed a file.
pub fn validate_candidate(
    bootstrap: &Bootstrap,
    candidate: Candidate,
) -> Result<ValidatedCandidate, ToolError> {
    let expected_snapshot = snapshot_digest(bootstrap)?;
    if candidate.schema_version != SCHEMA_VERSION
        || candidate.attempt_id != bootstrap.attempt_id
        || candidate.base_snapshot != expected_snapshot
    {
        return Err(ToolError::InvalidBootstrap);
    }
    if candidate.changed_files.len() > MAX_FILES || candidate.operations.len() > MAX_OPERATIONS {
        return Err(ToolError::ResourceLimit);
    }
    let successful = validate_receipts(bootstrap.grants, &candidate.operations)?;
    validate_names(
        candidate
            .changed_files
            .iter()
            .map(|file| file.path.as_str()),
    )?;
    let mut merged: BTreeMap<&str, &TextFile> = bootstrap
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut required = MutationCounts::default();
    let mut changed_bytes = 0;
    for file in &candidate.changed_files {
        validate_path(&file.path)?;
        if forbidden(&bootstrap.forbidden_paths, &file.path) {
            return Err(ToolError::ProtectedPath);
        }
        if !bootstrap
            .owned_paths
            .iter()
            .any(|pattern| pattern.matches(&file.path))
        {
            return Err(ToolError::OutsideScope);
        }
        validate_content(&file.content)?;
        changed_bytes += file.content.len();
        if changed_bytes > MAX_TOTAL_BYTES {
            return Err(ToolError::ResourceLimit);
        }
        if merged.contains_key(file.path.as_str()) {
            if !bootstrap.grants.edit_files {
                return Err(ToolError::Denied);
            }
            required.edits += 1;
        } else {
            if !bootstrap.grants.write_files {
                return Err(ToolError::Denied);
            }
            required.creates += 1;
        }
        merged.insert(&file.path, file);
    }
    if successful.creates < required.creates || successful.edits < required.edits {
        return Err(ToolError::InvalidBootstrap);
    }
    // Reuse construction's final name/scope/content/size checks. In particular,
    // new names must not alias an unchanged file or one of its parent components.
    if merged.len() > MAX_FILES {
        return Err(ToolError::ResourceLimit);
    }
    let mut final_snapshot = bootstrap.clone();
    final_snapshot.files = merged.into_values().cloned().collect();
    validate_bootstrap(&final_snapshot)?;
    Ok(ValidatedCandidate {
        changed_files: candidate.changed_files,
    })
}

fn validate_receipts(
    grants: WritableGrant,
    receipts: &[OperationReceipt],
) -> Result<MutationCounts, ToolError> {
    let mut successful = MutationCounts::default();
    for (index, receipt) in receipts.iter().enumerate() {
        if receipt.sequence != index as u64 + 1 {
            return Err(ToolError::InvalidBootstrap);
        }
        let granted = match receipt.operation.as_str() {
            "read_file" => grants.read_files,
            "search_paths" => grants.search_files,
            "create_file" => grants.write_files,
            "edit_file" => grants.edit_files,
            _ => return Err(ToolError::InvalidBootstrap),
        };
        match receipt.status.as_str() {
            "succeeded" => {
                if receipt.code.is_some() {
                    return Err(ToolError::InvalidBootstrap);
                }
                if !granted {
                    return Err(ToolError::Denied);
                }
                match receipt.operation.as_str() {
                    "create_file" => successful.creates += 1,
                    "edit_file" => successful.edits += 1,
                    _ => {}
                }
            }
            "refused" => {
                let Some(code) = &receipt.code else {
                    return Err(ToolError::InvalidBootstrap);
                };
                let denied = code == ToolError::Denied.code();
                let expected_denial = !granted;
                if !REFUSAL_CODES.iter().any(|error| error.code() == code)
                    || denied != expected_denial
                {
                    return Err(ToolError::InvalidBootstrap);
                }
            }
            "outcome_unknown" => return Err(ToolError::Quarantined),
            _ => return Err(ToolError::InvalidBootstrap),
        }
    }
    Ok(successful)
}
