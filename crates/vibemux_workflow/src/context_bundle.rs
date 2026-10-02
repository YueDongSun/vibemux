//! Scoped, immutable context bundles (ADR 031 §5).
//!
//! A bundle carries selected line ranges of one task's collected candidate
//! snapshot, plus attributed claims, to a named audience. It is built from
//! observed evidence only: a worker's statement stays labeled as that
//! worker's claim and a verifier-confirmed fact carries its receipt digest,
//! so a summary can never turn a proposal into a confirmed decision. Secret
//! looking values are redacted before the byte budget is checked, and an
//! over-budget selection is refused rather than truncated.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    Sha256Digest, SpecIdentifier,
    messages::{Participant, SourceFileRef},
};

pub const CONTEXT_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const BUNDLE_DIGEST_DOMAIN: &str = "vibemux.workflow.context_bundle.v1";
pub const MAX_SELECTIONS: usize = 16;
pub const MAX_CLAIMS: usize = 16;
pub const MAX_CLAIM_BYTES: usize = 2048;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileSelection {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Attribution {
    /// An unverified statement by the named task's worker.
    WorkerClaim { task_key: SpecIdentifier },
    /// A fact the trusted verifier observed, with its receipt digest.
    VerifierConfirmed { receipt_digest: Sha256Digest },
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttributedClaim {
    pub attribution: Attribution,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileExcerpt {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionReport {
    pub rules_applied: Vec<String>,
    pub redacted_values: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextBundle {
    pub schema_version: u32,
    pub bundle_id: Uuid,
    pub version: u32,
    pub workflow_id: Uuid,
    pub source: Participant,
    pub source_task: SpecIdentifier,
    pub base_commit: String,
    pub snapshot_digest: Sha256Digest,
    pub audience: Vec<Participant>,
    pub request_message_id: Option<Uuid>,
    pub source_refs: Vec<SourceFileRef>,
    pub excerpts: Vec<FileExcerpt>,
    pub claims: Vec<AttributedClaim>,
    pub redaction: RedactionReport,
    pub byte_count: u64,
    pub content_digest: Sha256Digest,
}

/// The candidate snapshot a bundle is drawn from: text files by path.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotView<'a> {
    pub task_key: &'a SpecIdentifier,
    pub source: &'a Participant,
    pub snapshot_digest: Sha256Digest,
    pub base_commit: &'a str,
    pub files: &'a BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct BundleRequest<'a> {
    pub bundle_id: Uuid,
    pub workflow_id: Uuid,
    pub request_message_id: Option<Uuid>,
    pub selections: &'a [FileSelection],
    pub claims: &'a [AttributedClaim],
    pub audience: Vec<Participant>,
    pub max_bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BundleError {
    #[error("selection is outside the snapshot or its line range is invalid")]
    InvalidSelection,
    #[error("too many selections or claims")]
    TooManyItems,
    #[error("claim text is empty or too large")]
    InvalidClaim,
    #[error("bundle exceeds its byte budget")]
    OverBudget,
    #[error("bundle needs an audience")]
    NoAudience,
}

impl BundleError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidSelection => "bundle_invalid_selection",
            Self::TooManyItems => "bundle_too_many_items",
            Self::InvalidClaim => "bundle_invalid_claim",
            Self::OverBudget => "bundle_over_budget",
            Self::NoAudience => "bundle_no_audience",
        }
    }
}

pub fn build_bundle(
    request: &BundleRequest<'_>,
    snapshot: SnapshotView<'_>,
) -> Result<ContextBundle, BundleError> {
    if request.selections.len() > MAX_SELECTIONS || request.claims.len() > MAX_CLAIMS {
        return Err(BundleError::TooManyItems);
    }
    if request.audience.is_empty() {
        return Err(BundleError::NoAudience);
    }
    let mut redaction = RedactionReport::default();
    let mut excerpts = Vec::new();
    let mut source_refs = Vec::new();
    let mut selections = request.selections.to_vec();
    selections.sort();
    selections.dedup();
    for selection in &selections {
        let text = snapshot
            .files
            .get(&selection.path)
            .ok_or(BundleError::InvalidSelection)?;
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let start =
            usize::try_from(selection.start_line).map_err(|_| BundleError::InvalidSelection)?;
        let end = usize::try_from(selection.end_line).map_err(|_| BundleError::InvalidSelection)?;
        if start == 0 || end < start || end > lines.len() {
            return Err(BundleError::InvalidSelection);
        }
        let raw: String = lines[start - 1..end].concat();
        let (clean, count, rules) = redact(&raw);
        redaction.redacted_values += count;
        redaction.rules_applied.extend(rules);
        excerpts.push(FileExcerpt {
            path: selection.path.clone(),
            start_line: selection.start_line,
            end_line: selection.end_line,
            text: clean,
        });
        source_refs.push(SourceFileRef {
            path: selection.path.clone(),
            start_line: selection.start_line,
            end_line: selection.end_line,
            snapshot_digest: snapshot.snapshot_digest,
        });
    }
    let mut claims = Vec::new();
    for claim in request.claims {
        if claim.text.trim().is_empty() || claim.text.len() > MAX_CLAIM_BYTES {
            return Err(BundleError::InvalidClaim);
        }
        let (clean, count, rules) = redact(&claim.text);
        redaction.redacted_values += count;
        redaction.rules_applied.extend(rules);
        claims.push(AttributedClaim {
            attribution: claim.attribution.clone(),
            text: clean,
        });
    }
    redaction.rules_applied.sort();
    redaction.rules_applied.dedup();
    let mut audience = request.audience.clone();
    audience.sort();
    audience.dedup();
    let mut bundle = ContextBundle {
        schema_version: CONTEXT_BUNDLE_SCHEMA_VERSION,
        bundle_id: request.bundle_id,
        version: 1,
        workflow_id: request.workflow_id,
        source: snapshot.source.clone(),
        source_task: snapshot.task_key.clone(),
        base_commit: snapshot.base_commit.to_string(),
        snapshot_digest: snapshot.snapshot_digest,
        audience,
        request_message_id: request.request_message_id,
        source_refs,
        excerpts,
        claims,
        redaction,
        byte_count: 0,
        content_digest: Sha256Digest::of(b""),
    };
    let text = render_bundle_text(&bundle);
    if text.len() > usize::try_from(request.max_bytes).unwrap_or(usize::MAX) {
        return Err(BundleError::OverBudget);
    }
    bundle.byte_count = text.len() as u64;
    bundle.content_digest = Sha256Digest::of_fields(BUNDLE_DIGEST_DOMAIN, &[text.as_bytes()]);
    Ok(bundle)
}

/// The deterministic text a recipient receives; it is also what the
/// content digest covers.
#[must_use]
pub fn render_bundle_text(bundle: &ContextBundle) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "Context from task {} (snapshot {}, base {}).\n",
        bundle.source_task, bundle.snapshot_digest, bundle.base_commit
    ));
    for claim in &bundle.claims {
        let label = match &claim.attribution {
            Attribution::WorkerClaim { task_key } => {
                format!("claim by worker {task_key} (unverified)")
            }
            Attribution::VerifierConfirmed { receipt_digest } => {
                format!("verifier-confirmed (receipt {receipt_digest})")
            }
        };
        text.push_str(&format!("- {label}: {}\n", claim.text));
    }
    for excerpt in &bundle.excerpts {
        text.push_str(&format!(
            "excerpt {} lines {}-{}:\n",
            excerpt.path, excerpt.start_line, excerpt.end_line
        ));
        text.push_str(&excerpt.text);
        if !excerpt.text.ends_with('\n') {
            text.push('\n');
        }
    }
    text
}

/// Replaces secret-looking values with `[redacted:<rule>]`. Returns the
/// text, the number of values replaced, and the rules that fired.
#[must_use]
pub fn redact(text: &str) -> (String, u32, Vec<String>) {
    let mut output = text.to_string();
    let mut count = 0;
    let mut rules = Vec::new();
    if let Some(start) = output.find("-----BEGIN") {
        if let Some(offset) = output[start..].find("PRIVATE KEY-----") {
            let header_end = start + offset + "PRIVATE KEY-----".len();
            let end = output[header_end..]
                .find("-----END")
                .and_then(|end_offset| {
                    let end_start = header_end + end_offset;
                    output[end_start..]
                        .find("-----\n")
                        .map(|tail| end_start + tail + 5)
                        .or(Some(output.len()))
                })
                .unwrap_or(output.len());
            output.replace_range(start..end, "[redacted:private_key]");
            count += 1;
            rules.push("private_key".to_string());
        }
    }
    for (rule, prefix, minimum, allowed) in TOKEN_RULES {
        let mut search = 0;
        while let Some(position) = output[search..].find(prefix) {
            let start = search + position;
            let value_start = start + prefix.len();
            let value_len = output[value_start..]
                .bytes()
                .take_while(|byte| allowed(*byte))
                .count();
            let boundary = output[..start]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_ascii_alphanumeric());
            if boundary && value_len >= *minimum {
                let replacement = format!("[redacted:{rule}]");
                output.replace_range(start..value_start + value_len, &replacement);
                count += 1;
                rules.push((*rule).to_string());
                search = start + replacement.len();
            } else {
                search = value_start;
            }
        }
    }
    let (assigned, assigned_count) = redact_assignments(&output);
    if assigned_count > 0 {
        output = assigned;
        count += assigned_count;
        rules.push("credential_assignment".to_string());
    }
    (output, count, rules)
}

type TokenRule = (&'static str, &'static str, usize, fn(u8) -> bool);

fn token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn bearer_byte(byte: u8) -> bool {
    byte.is_ascii_graphic()
}

fn upper_alnum(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}

const TOKEN_RULES: &[TokenRule] = &[
    ("api_key", "sk-", 20, token_byte),
    ("github_token", "ghp_", 20, token_byte),
    ("github_token", "github_pat_", 20, token_byte),
    ("aws_access_key", "AKIA", 16, upper_alnum),
    ("bearer_token", "Bearer ", 16, bearer_byte),
];

/// `name = value` or `name: value` where the name mentions a credential
/// and the value is at least eight non-space characters.
fn redact_assignments(text: &str) -> (String, u32) {
    const NAMES: &[&str] = &[
        "api_key",
        "apikey",
        "secret",
        "password",
        "passwd",
        "token",
        "credential",
    ];
    let mut count = 0;
    let mut lines = Vec::new();
    for line in text.split_inclusive('\n') {
        let lower = line.to_ascii_lowercase();
        let separator = lower.find('=').or_else(|| lower.find(':'));
        let rewritten = separator.and_then(|index| {
            let name = &lower[..index];
            if !NAMES.iter().any(|candidate| name.contains(candidate)) {
                return None;
            }
            let rest = &line[index + 1..];
            let value = rest.trim().trim_matches(['"', '\'', ',', ';']);
            (value.len() >= 8 && !value.contains(' ') && !value.starts_with("[redacted")).then(
                || {
                    let newline = if line.ends_with('\n') { "\n" } else { "" };
                    format!(
                        "{}[redacted:credential_assignment]{newline}",
                        &line[..=index]
                    )
                },
            )
        });
        match rewritten {
            Some(replacement) => {
                count += 1;
                lines.push(replacement);
            }
            None => lines.push(line.to_string()),
        }
    }
    (lines.concat(), count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::fixtures::worker;

    fn snapshot_files() -> BTreeMap<String, String> {
        let mut files = BTreeMap::new();
        files.insert(
            "src/server.mjs".to_string(),
            "line one\nif (typeof body.title !== 'string') return invalid_request;\nconst title = body.title.trim();\nif (!title || [...title].length > 120) return invalid_title;\nconst API_KEY = \"sk-abcdefghijklmnopqrstuvwx\";\n".to_string(),
        );
        files
    }

    fn request<'a>(
        selections: &'a [FileSelection],
        claims: &'a [AttributedClaim],
        max_bytes: u32,
    ) -> BundleRequest<'a> {
        BundleRequest {
            bundle_id: Uuid::from_u128(5),
            workflow_id: Uuid::from_u128(9),
            request_message_id: Some(Uuid::from_u128(77)),
            selections,
            claims,
            audience: vec![worker("track_b", 2)],
            max_bytes,
        }
    }

    fn build(
        selections: &[FileSelection],
        claims: &[AttributedClaim],
        max_bytes: u32,
    ) -> Result<ContextBundle, BundleError> {
        let files = snapshot_files();
        let task = SpecIdentifier::new("track_a").expect("key");
        let source = worker("track_a", 1);
        build_bundle(
            &request(selections, claims, max_bytes),
            SnapshotView {
                task_key: &task,
                source: &source,
                snapshot_digest: Sha256Digest::of(b"snapshot_a2"),
                base_commit: "abc",
                files: &files,
            },
        )
    }

    #[test]
    fn bundles_carry_source_refs_attribution_and_redaction() {
        let selections = [FileSelection {
            path: "src/server.mjs".into(),
            start_line: 2,
            end_line: 5,
        }];
        let claims = [AttributedClaim {
            attribution: Attribution::WorkerClaim { task_key: SpecIdentifier::new("track_a").expect("key") },
            text: "A non-string title is invalid_request; a blank or long string title is invalid_title.".into(),
        }];
        let bundle = build(&selections, &claims, 8192).expect("bundle");
        assert_eq!(
            bundle.source_refs[0].snapshot_digest,
            Sha256Digest::of(b"snapshot_a2")
        );
        let text = render_bundle_text(&bundle);
        assert!(text.contains("claim by worker track_a (unverified)"));
        assert!(text.contains("excerpt src/server.mjs lines 2-5:\nif (typeof body.title"));
        assert!(!text.contains("sk-abcdefghijklmnopqrstuvwx"));
        assert!(bundle.redaction.redacted_values >= 1);
        assert_eq!(bundle.byte_count, text.len() as u64);
        assert_eq!(build(&selections, &claims, 8192).expect("again"), bundle);
    }

    #[test]
    fn invalid_selections_and_budgets_are_refused_not_truncated() {
        for selection in [
            FileSelection {
                path: "src/missing.mjs".into(),
                start_line: 1,
                end_line: 1,
            },
            FileSelection {
                path: "src/server.mjs".into(),
                start_line: 0,
                end_line: 1,
            },
            FileSelection {
                path: "src/server.mjs".into(),
                start_line: 4,
                end_line: 3,
            },
            FileSelection {
                path: "src/server.mjs".into(),
                start_line: 1,
                end_line: 99,
            },
        ] {
            assert_eq!(
                build(&[selection], &[], 8192),
                Err(BundleError::InvalidSelection)
            );
        }
        let selection = [FileSelection {
            path: "src/server.mjs".into(),
            start_line: 1,
            end_line: 5,
        }];
        assert_eq!(build(&selection, &[], 64), Err(BundleError::OverBudget));
    }

    #[test]
    fn redaction_covers_common_secret_shapes_without_eating_code() {
        let text = "token = abcdefgh12345\nAuthorization: Bearer abcdefghijklmnopqrstu\nkey AKIAABCDEFGHIJKLMNOP\nconst limit = 8192;\nghp_abcdefghijklmnopqrstuvwxyz\n-----BEGIN RSA PRIVATE KEY-----\nMIIB\n-----END RSA PRIVATE KEY-----\nkeep me\n";
        let (clean, count, rules) = redact(text);
        for secret in [
            "abcdefgh12345",
            "abcdefghijklmnopqrstu",
            "AKIAABCDEFGHIJKLMNOP",
            "ghp_abcdef",
            "MIIB",
        ] {
            assert!(!clean.contains(secret), "{secret} leaked in {clean}");
        }
        assert!(clean.contains("const limit = 8192;"));
        assert!(clean.contains("keep me"));
        assert!(count >= 5);
        assert!(rules.contains(&"private_key".to_string()));
    }
}
