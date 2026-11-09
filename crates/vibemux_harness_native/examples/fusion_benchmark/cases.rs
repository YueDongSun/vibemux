//! Fixed coding-reasoning tasks with model-independent, in-process grading.
//!
//! Candidate text is data. The scorer does not compile or execute it. These
//! tasks measure constrained patch selection and boundary reasoning, not
//! repository editing, arbitrary program synthesis, or a hidden benchmark.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub const SUITE_VERSION: &str = "fusion_coding_reasoning_v1";
const MAX_ANSWER_BYTES: usize = 16 * 1024;

pub struct Case {
    pub id: &'static str,
    pub prompt: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Grade {
    pub checks_passed: usize,
    pub checks_total: usize,
    pub passed: bool,
}

impl Grade {
    fn from_checks(checks: impl IntoIterator<Item = bool>) -> Self {
        let mut checks_passed = 0;
        let mut checks_total = 0;
        for passed in checks {
            checks_total += 1;
            checks_passed += usize::from(passed);
        }
        Self {
            checks_passed,
            checks_total,
            passed: checks_passed == checks_total,
        }
    }
}

const PAGINATION: &str = r#"Review this defective keyset pagination implementation:
  eligible = rows.filter(row => !cursor || row.time >= cursor.time);
  page = eligible.sort((a,b) => a.time-b.time).slice(0, limit);
  next = page.length ? [page.at(-1).time,page.at(-1).id] : null;

The required API sorts ascending by (time,id), excludes every row at or before
the cursor tuple, and accepts only integer limits from 1 through 3 inclusive.
next is the last returned [time,id] only when a later eligible row exists;
otherwise next is null. Invalid limits return empty ids, null next and
error="invalid_limit". All successful queries use error=null.

Choose the minimal patch settings from these alternatives:
  order: "time_only" or "time_then_id"
  cursor: "time_inclusive", "time_exclusive", or "tuple_exclusive"
  fetch: "exactly_limit" or "limit_plus_one"

Data in insertion order, as [id,time]:
  [[7,20],[3,10],[8,30],[2,10],[5,20],[1,0],[6,20],[4,15]]
Evaluate these eight independent queries after fixing the implementation;
cursor tuples are [time,id]:
  1. cursor=null, limit=3
  2. cursor=[10,2], limit=2
  3. cursor=[20,5], limit=3
  4. cursor=[20,7], limit=1
  5. cursor=[30,8], limit=3
  6. cursor=null, limit=0
  7. cursor=null, limit=4
  8. cursor=[5,99], limit=2

Return exactly one JSON object, without Markdown or explanation:
{"patch":{"order":CHOICE,"cursor":CHOICE,"fetch":CHOICE},
 "pages":[{"ids":[INTEGER_IDS],"next":TUPLE_OR_NULL,"error":STRING_OR_NULL}, ...]}
pages must contain exactly eight objects in query order. Do not call tools."#;

const IDEMPOTENCY: &str = r#"Review this defective transfer routine:
  if seen[key]: return replay;
  from.balance -= amount;
  to.balance += amount;
  seen[key] = request;
  append_event(request);

Required behavior: the idempotency identity is (tenant,key). A committed
identity with identical (from,to,amount) returns replay; different data returns
conflict, regardless of current balances. Otherwise insufficient funds returns
insufficient without reserving the key or changing any state. Successful
balance updates, the idempotency row and one event commit atomically. An
injected failure immediately before commit returns injected_failure and rolls
back all three. The failure flag itself is not part of request identity.
There are no negative amounts, unknown accounts, or concurrent calls here.

Choose patch settings:
  identity: "key_only" or "tenant_and_key"
  atomicity: "state_then_key_then_event" or "state_key_event_one_transaction"

Initial balances are A=20, B=5, C=0. No prior keys or events exist.
Evaluate this sequential trace, where each row is
[tenant,key,from,to,amount,fail_before_commit]:
  1. ["t1","k","A","B",6,false]
  2. ["t1","k","A","B",6,false]
  3. ["t1","k","A","B",7,false]
  4. ["t2","k","B","C",4,false]
  5. ["t1","k2","C","A",5,false]
  6. ["t1","k3","B","C",3,true]
  7. ["t1","k3","B","C",3,false]
  8. ["t1","k2","C","A",5,false]
  9. ["t1","k2","C","A",5,false]
 10. ["t2","k","B","C",4,false]

Return exactly one JSON object, without Markdown or explanation:
{"patch":{"identity":CHOICE,"atomicity":CHOICE},
 "responses":[TEN_STATUS_STRINGS],"balances":[A,B,C],"committed_events":INTEGER}
Allowed statuses: "applied", "replay", "conflict", "insufficient",
"injected_failure". Do not call tools."#;

const FRAMING: &str = r#"Review a defective newline-framed JSON reader:
  for chunk in chunks:
    text = utf8_decode(chunk)
    for line in text.split("\n"):
      if character_count(line) > limit: fail(frame_too_large)
      emit(parse_json(line))

Required behavior: buffer bytes across chunks. LF terminates a frame; remove
one final CR before checking the payload byte limit. At EOF parse a nonempty
tail as a frame; a trailing LF does not create an extra empty frame. On each
complete frame check, in order: payload byte limit, empty payload, UTF-8,
JSON syntax, then top-level object type. Stop at the first failed frame.
Count only successfully emitted frames and their payload bytes, excluding CR/LF.
Do not treat braces or Unicode code points as transport boundaries. Limits
below are inclusive, and every stream ends after its final listed chunk.

Choose patch settings:
  decoding: "decode_each_chunk" or "buffer_bytes_then_decode"
  limit_unit: "characters" or "bytes"
  eof: "discard_tail" or "parse_nonempty_tail"

Eight streams, with chunks encoded as hexadecimal bytes:
  1. limit=14, chunks=["7b2274223a22e4","bda0e5","a5bd227d0a"]
  2. limit=13, chunks=["7b2274223a22e4","bda0e5","a5bd227d0a"]
  3. limit=8, chunks=["7b7d0d","0a7b226e223a317d0a"]
  4. limit=8, chunks=["7b2278223aff7d0a"]
  5. limit=8, chunks=["7b7d0a747275650a"]
  6. limit=8, chunks=["7b226e223a317d"]
  7. limit=8, chunks=["0a"]
  8. limit=8, chunks=["7b7d0a7b"]

Return exactly one JSON object, without Markdown or explanation:
{"patch":{"decoding":CHOICE,"limit_unit":CHOICE,"eof":CHOICE},
 "streams":[{"status":STATUS,"frames":INTEGER,"payload_bytes":INTEGER}, ...]}
streams must contain exactly eight objects in input order. Allowed statuses:
"ok", "frame_too_large", "empty_frame", "invalid_utf8", "invalid_json",
"non_object". Do not call tools."#;

pub fn cases() -> Vec<Case> {
    vec![
        Case {
            id: "keyset_pagination",
            prompt: PAGINATION,
        },
        Case {
            id: "idempotent_transfer",
            prompt: IDEMPOTENCY,
        },
        Case {
            id: "stream_framing",
            prompt: FRAMING,
        },
    ]
}

/// Pin the exact prompts, fixtures and executable rubric together. Deliberate
/// source edits, including reference or contract-test edits, change the digest.
pub fn suite_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(SUITE_VERSION.as_bytes());
    hasher.update([0]);
    hasher.update(include_bytes!("cases.rs"));
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in hasher.finalize() {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
    output
}

fn parse<T: DeserializeOwned>(answer: &str) -> Result<T, &'static str> {
    if answer.is_empty() || answer.len() > MAX_ANSWER_BYTES {
        return Err("fusion_answer_size");
    }
    // Deserialize straight into fixed structs: duplicate/unknown fields and
    // trailing prose are errors rather than silently accepted JSON values.
    serde_json::from_str(answer).map_err(|_| "fusion_answer_schema")
}

pub fn grade(case_id: &str, answer: &str) -> Result<Grade, &'static str> {
    match case_id {
        "keyset_pagination" => grade_pagination(parse(answer)?),
        "idempotent_transfer" => Ok(grade_transfer(parse(answer)?)),
        "stream_framing" => Ok(grade_framing(parse(answer)?)),
        _ => Err("fusion_unknown_case"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaginationAnswer {
    patch: PaginationPatch,
    pages: [Page; 8],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PaginationPatch {
    order: Order,
    cursor: Cursor,
    fetch: Fetch,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Order {
    TimeOnly,
    TimeThenId,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Cursor {
    TimeInclusive,
    TimeExclusive,
    TupleExclusive,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Fetch {
    ExactlyLimit,
    LimitPlusOne,
}

#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Page {
    ids: Vec<u16>,
    #[serde(deserialize_with = "required_nullable")]
    next: Option<[i64; 2]>,
    #[serde(deserialize_with = "required_nullable")]
    error: Option<PageError>,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PageError {
    InvalidLimit,
}

fn grade_pagination(answer: PaginationAnswer) -> Result<Grade, &'static str> {
    if answer.pages.iter().any(|page| page.ids.len() > 3) {
        return Err("fusion_answer_schema");
    }
    let queries = [
        (None, 3),
        (Some((10, 2)), 2),
        (Some((20, 5)), 3),
        (Some((20, 7)), 1),
        (Some((30, 8)), 3),
        (None, 0),
        (None, 4),
        (Some((5, 99)), 2),
    ];
    let patch_checks = [
        answer.patch.order == Order::TimeThenId,
        answer.patch.cursor == Cursor::TupleExclusive,
        answer.patch.fetch == Fetch::LimitPlusOne,
    ];
    Ok(Grade::from_checks(
        patch_checks.into_iter().chain(
            answer
                .pages
                .iter()
                .zip(queries)
                .map(|(page, (cursor, limit))| *page == reference_page(cursor, limit)),
        ),
    ))
}

fn reference_page(cursor: Option<(i64, u16)>, limit: usize) -> Page {
    if !(1..=3).contains(&limit) {
        return Page {
            ids: vec![],
            next: None,
            error: Some(PageError::InvalidLimit),
        };
    }
    // Independent tuple-order oracle; storage insertion order is immaterial.
    let mut rows = [
        (20, 7),
        (10, 3),
        (30, 8),
        (10, 2),
        (20, 5),
        (0, 1),
        (20, 6),
        (15, 4),
    ];
    rows.sort_unstable();
    let eligible: Vec<_> = rows
        .into_iter()
        .filter(|row| cursor.is_none_or(|cursor| *row > cursor))
        .collect();
    let selected: Vec<_> = eligible.iter().take(limit).collect();
    Page {
        ids: selected.iter().map(|row| row.1).collect(),
        next: if eligible.len() > limit {
            selected.last().map(|row| [row.0, i64::from(row.1)])
        } else {
            None
        },
        error: None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferAnswer {
    patch: TransferPatch,
    responses: [TransferStatus; 10],
    balances: [i64; 3],
    committed_events: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferPatch {
    identity: Identity,
    atomicity: Atomicity,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Identity {
    KeyOnly,
    TenantAndKey,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Atomicity {
    StateThenKeyThenEvent,
    StateKeyEventOneTransaction,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TransferStatus {
    Applied,
    Replay,
    Conflict,
    Insufficient,
    InjectedFailure,
}

fn grade_transfer(answer: TransferAnswer) -> Grade {
    use TransferStatus::{Applied, Conflict, InjectedFailure, Insufficient, Replay};
    // Independent reference trace: only calls 1, 4, 7, 8 commit. Their deltas
    // are A->B 6, B->C 4, B->C 3, C->A 5; all other calls are state-neutral.
    let responses = [
        Applied,
        Replay,
        Conflict,
        Applied,
        Insufficient,
        InjectedFailure,
        Applied,
        Applied,
        Replay,
        Replay,
    ];
    let balances = [20 - 6 + 5, 5 + 6 - 4 - 3, 4 + 3 - 5];
    Grade::from_checks(
        [
            answer.patch.identity == Identity::TenantAndKey,
            answer.patch.atomicity == Atomicity::StateKeyEventOneTransaction,
            answer.committed_events == 4,
        ]
        .into_iter()
        .chain(
            answer
                .responses
                .iter()
                .zip(responses)
                .map(|(got, want)| *got == want),
        )
        .chain(
            answer
                .balances
                .iter()
                .zip(balances)
                .map(|(got, want)| *got == want),
        ),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FramingAnswer {
    patch: FramingPatch,
    streams: [Stream; 8],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FramingPatch {
    decoding: Decoding,
    limit_unit: LimitUnit,
    eof: Eof,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Decoding {
    DecodeEachChunk,
    BufferBytesThenDecode,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LimitUnit {
    Characters,
    Bytes,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Eof {
    DiscardTail,
    ParseNonemptyTail,
}

#[derive(Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Stream {
    status: FrameStatus,
    frames: u64,
    payload_bytes: u64,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum FrameStatus {
    Ok,
    FrameTooLarge,
    EmptyFrame,
    InvalidUtf8,
    InvalidJson,
    NonObject,
}

fn grade_framing(answer: FramingAnswer) -> Grade {
    use FrameStatus::{EmptyFrame, FrameTooLarge, InvalidJson, InvalidUtf8, NonObject, Ok};
    // Totals count emitted payloads only. Invalid tails preserve prior totals.
    // The first payload is six ASCII prefix bytes, six UTF-8 bytes and two
    // ASCII suffix bytes; chunk-level UTF-8 decoding cannot handle this input.
    let expected = [
        (Ok, 1, 14),
        (FrameTooLarge, 0, 0),
        (Ok, 2, 9),
        (InvalidUtf8, 0, 0),
        (NonObject, 1, 2),
        (Ok, 1, 7),
        (EmptyFrame, 0, 0),
        (InvalidJson, 1, 2),
    ];
    Grade::from_checks(
        [
            answer.patch.decoding == Decoding::BufferBytesThenDecode,
            answer.patch.limit_unit == LimitUnit::Bytes,
            answer.patch.eof == Eof::ParseNonemptyTail,
        ]
        .into_iter()
        .chain(answer.streams.iter().zip(expected).map(
            |(got, (status, frames, payload_bytes))| {
                *got == Stream {
                    status,
                    frames,
                    payload_bytes,
                }
            },
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSFER_SOLUTION: &str = r#"{"patch":{"identity":"tenant_and_key","atomicity":"state_key_event_one_transaction"},"responses":["applied","replay","conflict","applied","insufficient","injected_failure","applied","applied","replay","replay"],"balances":[19,4,2],"committed_events":4}"#;
    const FRAMING_SOLUTION: &str = r#"{"patch":{"decoding":"buffer_bytes_then_decode","limit_unit":"bytes","eof":"parse_nonempty_tail"},"streams":[{"status":"ok","frames":1,"payload_bytes":14},{"status":"frame_too_large","frames":0,"payload_bytes":0},{"status":"ok","frames":2,"payload_bytes":9},{"status":"invalid_utf8","frames":0,"payload_bytes":0},{"status":"non_object","frames":1,"payload_bytes":2},{"status":"ok","frames":1,"payload_bytes":7},{"status":"empty_frame","frames":0,"payload_bytes":0},{"status":"invalid_json","frames":1,"payload_bytes":2}]}"#;
    const PAGINATION_SOLUTION: &str = r#"{"patch":{"order":"time_then_id","cursor":"tuple_exclusive","fetch":"limit_plus_one"},"pages":[{"ids":[1,2,3],"next":[10,3],"error":null},{"ids":[3,4],"next":[15,4],"error":null},{"ids":[6,7,8],"next":null,"error":null},{"ids":[8],"next":null,"error":null},{"ids":[],"next":null,"error":null},{"ids":[],"next":null,"error":"invalid_limit"},{"ids":[],"next":null,"error":"invalid_limit"},{"ids":[2,3],"next":[10,3],"error":null}]}"#;

    #[test]
    fn independently_worked_solutions_satisfy_every_check() {
        for (id, text, total) in [
            ("keyset_pagination", PAGINATION_SOLUTION, 11),
            ("idempotent_transfer", TRANSFER_SOLUTION, 16),
            ("stream_framing", FRAMING_SOLUTION, 11),
        ] {
            let grade = grade(id, text).expect("solution has the required shape");
            assert!(grade.passed, "{id}");
            assert_eq!(grade.checks_passed, total);
            assert_eq!(grade.checks_total, total);
        }
    }

    #[test]
    fn plausible_boundary_and_atomicity_mistakes_lose_credit() {
        for (id, text) in [
            (
                "keyset_pagination",
                PAGINATION_SOLUTION.replace("\"ids\":[3,4]", "\"ids\":[2,3]"),
            ),
            (
                "keyset_pagination",
                PAGINATION_SOLUTION.replacen("\"next\":null", "\"next\":[30,8]", 1),
            ),
            (
                "idempotent_transfer",
                TRANSFER_SOLUTION.replace("\"committed_events\":4", "\"committed_events\":5"),
            ),
            (
                "idempotent_transfer",
                TRANSFER_SOLUTION.replace("\"balances\":[19,4,2]", "\"balances\":[19,1,5]"),
            ),
            (
                "stream_framing",
                FRAMING_SOLUTION.replace("\"payload_bytes\":14", "\"payload_bytes\":10"),
            ),
            (
                "stream_framing",
                FRAMING_SOLUTION.replace(
                    "\"invalid_json\",\"frames\":1",
                    "\"invalid_json\",\"frames\":0",
                ),
            ),
        ] {
            assert!(!grade(id, &text).expect("valid but incorrect answer").passed);
        }
    }

    #[test]
    fn malformed_or_ambiguous_answers_cannot_be_scored_as_success() {
        for text in [
            format!("```json\n{TRANSFER_SOLUTION}\n```"),
            format!("{TRANSFER_SOLUTION}\nThe answer is correct."),
            TRANSFER_SOLUTION.replace(
                "\"balances\":[19,4,2]",
                "\"balances\":[19,4,2],\"balances\":[19,4,2]",
            ),
            TRANSFER_SOLUTION.replace(
                "\"identity\":\"tenant_and_key\"",
                "\"identity\":\"tenant_and_key\",\"identity\":\"key_only\"",
            ),
            TRANSFER_SOLUTION.replace(
                "\"committed_events\":4",
                "\"committed_events\":4,\"grade\":\"passed\"",
            ),
            TRANSFER_SOLUTION.replace("\"balances\":[19,4,2]", "\"balances\":[19,4]"),
            TRANSFER_SOLUTION.replace("\"replay\"", "\"maybe\""),
        ] {
            assert_eq!(
                grade("idempotent_transfer", &text).unwrap_err(),
                "fusion_answer_schema"
            );
        }
        assert_eq!(
            grade("idempotent_transfer", &" ".repeat(MAX_ANSWER_BYTES + 1)).unwrap_err(),
            "fusion_answer_size"
        );
        assert_eq!(
            grade("not_a_case", "{}").unwrap_err(),
            "fusion_unknown_case"
        );
    }

    #[test]
    fn nullable_fields_are_required_and_page_lengths_are_bounded() {
        for text in [
            PAGINATION_SOLUTION.replacen("\"next\":null,", "", 1),
            PAGINATION_SOLUTION.replacen(",\"error\":null", "", 1),
            PAGINATION_SOLUTION.replacen("\"ids\":[1,2,3]", "\"ids\":[1,2,3,4]", 1),
        ] {
            assert_eq!(
                grade("keyset_pagination", &text).unwrap_err(),
                "fusion_answer_schema"
            );
        }
    }

    #[test]
    fn prompts_have_unique_names_and_source_identity_is_bounded() {
        let suite = cases();
        let names: std::collections::BTreeSet<_> = suite.iter().map(|case| case.id).collect();
        assert_eq!(suite.len(), names.len());
        assert!(
            suite
                .iter()
                .all(|case| !case.prompt.is_empty() && case.prompt.len() < MAX_ANSWER_BYTES)
        );
        let digest = suite_digest();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
