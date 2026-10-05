//! Content-free provenance for owned paths in an admitted TaskSpec.
//!
//! Containment in a writable root is a ceiling, not a source of new ownership.
//! Every owned pattern needs an exact source literal in a positive requirement,
//! or the identical pattern declared by the operator. Existing structural,
//! coverage, and policy checks still apply. This module performs no filesystem
//! access, glob expansion, case normalization of grants, or interpretation of
//! quoted source as commands. Source provenance does not replace semantic review.
//!
//! The existing PathPattern domain remains authoritative: UTF-8 source and
//! literals are preserved, but Unicode owned paths currently remain unsupported.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    PathPattern, Sha256Digest, SpecIdentifier,
    canonical_json::{CanonicalError, canonical_digest},
    compiler_validation::{SharedContractText, validate_coverage},
    policy::{OperatorPolicy, intersect},
    source_request::{ByteRange, SourceRequest, literal_tokens},
    task_spec::{NormativeForce, Requirement, SourceSpan, TaskSpec},
};

pub const PATH_AUTHORITY_SCHEMA_VERSION: u32 = 1;
const PATH_AUTHORITY_DOMAIN: &str = "vibemux.workflow.path_authority.v1";
const SPAN_IDENTITY_DOMAIN: &str = "vibemux.workflow.path_authority_span.v1";

/// Identity of exact UTF-8 source bytes, without retaining the quoted content.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpanIdentity {
    pub start: u32,
    pub end: u32,
    pub span_digest: Sha256Digest,
}

impl SourceSpanIdentity {
    pub fn from_source(source: &SourceRequest, span: &SourceSpan) -> Option<Self> {
        let range = source.resolve(span)?;
        let span_digest = Sha256Digest::of_fields(
            SPAN_IDENTITY_DOMAIN,
            &[
                source.digest().as_bytes(),
                &span.start.to_be_bytes(),
                &span.end.to_be_bytes(),
                source.text()[range.start..range.end].as_bytes(),
            ],
        );
        Some(Self {
            start: span.start,
            end: span.end,
            span_digest,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum PathProvenance {
    SourceRequirement {
        requirement_id: SpecIdentifier,
        source_span: SourceSpanIdentity,
    },
    OperatorPolicy {
        policy_digest: Sha256Digest,
        exact_pattern: PathPattern,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathAuthorityMap {
    pub schema_version: u32,
    pub task_spec_digest: Sha256Digest,
    pub source_request_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub owned_paths: BTreeMap<PathPattern, PathProvenance>,
}

impl PathAuthorityMap {
    pub fn digest(&self) -> Result<Sha256Digest, CanonicalError> {
        canonical_digest(PATH_AUTHORITY_DOMAIN, self)
    }

    /// Persisted metadata is not self-authenticating. Verify against the exact
    /// admitted inputs while those inputs are available at the trusted boundary.
    pub fn verify_inputs(
        &self,
        spec: &TaskSpec,
        source: &SourceRequest,
        policy: &OperatorPolicy,
        shared_contracts: &[SharedContractText<'_>],
    ) -> Result<(), Vec<PathAuthorityViolation>> {
        let expected = derive_path_authority(spec, source, policy, shared_contracts)?;
        if self == &expected {
            Ok(())
        } else {
            Err(vec![violation("path_authority_identity_mismatch", None)])
        }
    }
}

/// Codes and repository-relative patterns only; never original request text.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PathAuthorityViolation {
    pub code: &'static str,
    pub owned_path: Option<PathPattern>,
}

fn violation(code: &'static str, owned_path: Option<&PathPattern>) -> PathAuthorityViolation {
    PathAuthorityViolation {
        code,
        owned_path: owned_path.cloned(),
    }
}

/// Produces provenance for an already normalized, policy-narrowed TaskSpec.
/// A different case, child path, or broader wildcard is not an exact declaration.
pub fn derive_path_authority(
    spec: &TaskSpec,
    source: &SourceRequest,
    policy: &OperatorPolicy,
    shared_contracts: &[SharedContractText<'_>],
) -> Result<PathAuthorityMap, Vec<PathAuthorityViolation>> {
    spec.validate_structure().map_err(|errors| {
        errors
            .into_iter()
            .map(|error| violation(error.code, None))
            .collect::<Vec<_>>()
    })?;
    policy
        .validate()
        .map_err(|error| vec![violation(error.code, None)])?;
    validate_coverage(spec, source, shared_contracts).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| violation(error.code, None))
            .collect::<Vec<_>>()
    })?;
    let narrowed = intersect(spec.clone(), policy).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| violation(error.code, None))
            .collect::<Vec<_>>()
    })?;
    if narrowed.spec != *spec {
        return Err(vec![violation("path_authority_spec_not_narrowed", None)]);
    }
    let task_spec_digest = spec
        .digest()
        .map_err(|_| vec![violation("path_authority_invalid_identity", None)])?;
    let policy_digest = policy
        .digest()
        .map_err(|_| vec![violation("path_authority_invalid_identity", None)])?;
    let mut owned_paths = BTreeMap::new();
    let mut violations = BTreeSet::new();
    for path in &spec.owned_paths {
        // Match the existing ASCII Windows protection rule without widening the
        // case-sensitive owned-path domain or changing any original literal.
        if spec
            .forbidden_paths
            .iter()
            .chain(&policy.protected_paths)
            .any(|forbidden| protection_overlap(path, forbidden))
        {
            violations.insert(violation("path_authority_path_protected", Some(path)));
            continue;
        }
        if prohibited_path(path, spec, source) {
            violations.insert(violation("path_authority_path_prohibited", Some(path)));
            continue;
        }
        if let Some((requirement_id, source_span)) =
            source_witness(path.as_str(), &spec.requirements, source)
        {
            owned_paths.insert(
                path.clone(),
                PathProvenance::SourceRequirement {
                    requirement_id,
                    source_span,
                },
            );
        } else if policy
            .writable_roots
            .iter()
            .any(|declared| declared == path)
        {
            owned_paths.insert(
                path.clone(),
                PathProvenance::OperatorPolicy {
                    policy_digest,
                    exact_pattern: path.clone(),
                },
            );
        } else {
            violations.insert(violation(
                "path_authority_owned_path_unjustified",
                Some(path),
            ));
        }
    }
    if !violations.is_empty() {
        return Err(violations.into_iter().collect());
    }
    Ok(PathAuthorityMap {
        schema_version: PATH_AUTHORITY_SCHEMA_VERSION,
        task_spec_digest,
        source_request_digest: source.digest(),
        policy_digest,
        owned_paths,
    })
}

fn protection_overlap(left: &PathPattern, right: &PathPattern) -> bool {
    let lowered_left = PathPattern::new(left.as_str().to_ascii_lowercase());
    let lowered_right = PathPattern::new(right.as_str().to_ascii_lowercase());
    match (lowered_left, lowered_right) {
        (Ok(left), Ok(right)) => left.overlaps(&right),
        _ => true,
    }
}

fn prohibited_path(path: &PathPattern, spec: &TaskSpec, source: &SourceRequest) -> bool {
    let declared = spec
        .requirements
        .iter()
        .filter(|requirement| requirement.force == NormativeForce::MustNot)
        .flat_map(|requirement| &requirement.literals)
        .filter_map(|literal| PathPattern::new(literal.clone()).ok())
        .any(|forbidden| protection_overlap(path, &forbidden));
    if declared {
        return true;
    }
    let prohibitions = source.prohibition_markers();
    source
        .clauses()
        .into_iter()
        .filter(|clause| prohibitions.iter().any(|marker| clause.contains(*marker)))
        .flat_map(|clause| literal_tokens(source.text(), clause))
        .filter_map(|literal| PathPattern::new(literal).ok())
        .any(|forbidden| protection_overlap(path, &forbidden))
}

fn source_witness(
    path: &str,
    requirements: &[Requirement],
    source: &SourceRequest,
) -> Option<(SpecIdentifier, SourceSpanIdentity)> {
    let literals = exact_literal_ranges(source, path);
    let prohibitions = source.prohibition_markers();
    requirements
        .iter()
        .filter(|requirement| {
            requirement.force != NormativeForce::MustNot
                && requirement.literals.iter().any(|literal| literal == path)
        })
        .flat_map(|requirement| {
            requirement.source_refs.iter().filter_map(|span| {
                let range = source.resolve(span)?;
                if !literals.iter().any(|literal| range.contains(*literal))
                    || prohibitions.iter().any(|marker| range.overlaps(*marker))
                {
                    return None;
                }
                Some((
                    requirement.requirement_id.clone(),
                    SourceSpanIdentity::from_source(source, span)?,
                ))
            })
        })
        .min()
}

/// Locates complete literal tokens in the original bytes. Looking in a sliced
/// span alone would incorrectly accept `a.mjs` as a suffix of `other/a.mjs`.
fn exact_literal_ranges(source: &SourceRequest, path: &str) -> Vec<ByteRange> {
    let text = source.text();
    let quoted = source.literal_regions();
    let mut matches = Vec::new();
    for region in &quoted {
        let text_in_region = &text[region.start..region.end];
        let open = text_in_region.chars().next().map_or(0, char::len_utf8);
        let close = text_in_region.chars().next_back().map_or(0, char::len_utf8);
        let inner = ByteRange {
            start: region.start + open,
            end: region.end - close,
        };
        if text.get(inner.start..inner.end) == Some(path) {
            matches.push(inner);
        }
    }
    let mut offset = 0;
    for word in text.split_whitespace() {
        let Some(relative) = text[offset..].find(word) else {
            break;
        };
        let start = offset + relative;
        offset = start + word.len();
        let word_range = ByteRange { start, end: offset };
        if quoted.iter().any(|region| region.overlaps(word_range)) {
            continue;
        }
        let trimmed = word
            .trim_end_matches([',', ';', ':', '.', ')', '，', '。', '；', '：', '）'])
            .trim_start_matches(['(', '（']);
        if trimmed == path {
            let leading = word.len() - word.trim_start_matches(['(', '（']).len();
            matches.push(ByteRange {
                start: start + leading,
                end: start + leading + trimmed.len(),
            });
        }
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        policy::fixtures::taskboard_policy,
        task_spec::fixtures::{REQUEST, id, path, span, track_a_spec},
    };

    fn fixture() -> (TaskSpec, SourceRequest, OperatorPolicy) {
        let policy = taskboard_policy();
        let mut spec = track_a_spec();
        spec.owned_paths = vec![path("src/store.mjs"), path("tests/worker_a/**")];
        let spec = intersect(spec.normalized(), &policy).expect("narrow").spec;
        (
            spec,
            SourceRequest::new(REQUEST.as_bytes().to_vec()).expect("source"),
            policy,
        )
    }

    fn codes(result: Result<PathAuthorityMap, Vec<PathAuthorityViolation>>) -> Vec<&'static str> {
        result
            .expect_err("rejected")
            .into_iter()
            .map(|error| error.code)
            .collect()
    }

    #[test]
    fn source_requirement_and_exact_operator_pattern_are_attributed_without_content() {
        let (spec, source, policy) = fixture();
        let authority = derive_path_authority(&spec, &source, &policy, &[]).expect("authority");
        assert_eq!(
            authority.task_spec_digest,
            spec.digest().expect("spec identity")
        );
        assert!(
            matches!(authority.owned_paths[&path("src/store.mjs")], PathProvenance::SourceRequirement { ref requirement_id, .. } if requirement_id == &id("r04"))
        );
        assert_eq!(
            authority.owned_paths[&path("tests/worker_a/**")],
            PathProvenance::OperatorPolicy {
                policy_digest: policy.digest().expect("policy identity"),
                exact_pattern: path("tests/worker_a/**")
            }
        );
        let json = serde_json::to_string(&authority).expect("metadata");
        assert!(!json.contains("Use "));
        assert!(!json.contains("persistence"));
        assert!(!json.contains("Titles must"));
        authority
            .verify_inputs(&spec, &source, &policy, &[])
            .expect("verified");
    }

    #[test]
    fn broad_writable_root_does_not_authorize_model_invented_paths() {
        let (mut spec, source, policy) = fixture();
        spec.owned_paths.push(path("src/admin.mjs"));
        spec = spec.normalized();
        assert_eq!(
            codes(derive_path_authority(&spec, &source, &policy, &[])),
            vec!["path_authority_owned_path_unjustified"]
        );
        let mut explicit = policy;
        explicit.writable_roots.push(path("src/admin.mjs"));
        let authority =
            derive_path_authority(&spec, &source, &explicit, &[]).expect("explicit declaration");
        assert!(matches!(
            authority.owned_paths[&path("src/admin.mjs")],
            PathProvenance::OperatorPolicy { .. }
        ));
    }

    #[test]
    fn path_prefix_and_case_changes_do_not_expand_source_authority() {
        let (mut spec, source, policy) = fixture();
        for attempted in ["src/store.mjs.bak", "src/Store.mjs", "src/**"] {
            spec.owned_paths = vec![path(attempted)];
            // `src/**` itself is an exact policy declaration, so remove that
            // declaration for this case: a file literal cannot grant a tree.
            let mut scoped_policy = policy.clone();
            if attempted == "src/**" {
                scoped_policy.writable_roots = vec![path("src/store.mjs")];
            }
            assert!(
                derive_path_authority(&spec.clone().normalized(), &source, &scoped_policy, &[])
                    .is_err(),
                "{attempted}"
            );
        }
        spec.owned_paths = vec![path("src_evil/store.mjs")];
        assert!(
            codes(derive_path_authority(
                &spec.normalized(),
                &source,
                &policy,
                &[]
            ))
            .contains(&"owned_path_outside_policy")
        );
    }

    #[test]
    fn a_literal_suffix_and_a_false_utf8_span_cannot_grant_ownership() {
        let source =
            SourceRequest::new("Edit `other/src/store.mjs`.".as_bytes().to_vec()).expect("source");
        let mut requirement = track_a_spec().requirements.remove(3);
        requirement.source_refs = vec![span(source.text(), "src/store.mjs")];
        assert!(source_witness("src/store.mjs", &[requirement], &source).is_none());
        let source =
            SourceRequest::new("修改 `src/store.mjs`。".as_bytes().to_vec()).expect("source");
        let mut wrong = span(source.text(), source.text());
        wrong.start = 1;
        assert!(SourceSpanIdentity::from_source(&source, &wrong).is_none());
    }

    #[test]
    fn unicode_source_literal_identity_is_exact_without_widening_the_owned_path_domain() {
        let literal = "src/数据模块.mjs";
        let text = format!("修改 `{literal}`。");
        let source = SourceRequest::new(text.as_bytes().to_vec()).expect("UTF-8 source");
        let mut requirement = track_a_spec().requirements.remove(3);
        requirement.source_refs = vec![span(&text, &text)];
        requirement.literals = vec![literal.to_string()];
        let (_, witness) =
            source_witness(literal, &[requirement], &source).expect("exact Unicode source literal");
        assert_eq!(witness.end as usize, text.len());
        let serialized = serde_json::to_string(&witness).expect("identity only");
        assert!(!serialized.contains(literal));
        assert!(
            PathPattern::new(literal).is_err(),
            "Unicode owned paths remain unsupported by the existing domain"
        );
    }

    #[test]
    fn exact_operator_grants_do_not_bypass_coverage_or_protected_paths() {
        let (mut spec, source, mut policy) = fixture();
        policy.writable_roots.push(path("src/store.mjs"));
        spec.requirements
            .retain(|requirement| requirement.requirement_id != id("r03"));
        let failures = codes(derive_path_authority(&spec, &source, &policy, &[]));
        assert!(failures.contains(&"dropped_prohibition"));
        let (mut spec, source, mut policy) = fixture();
        policy.writable_roots.push(path("VERIFIER/**"));
        spec.owned_paths = vec![path("VERIFIER/check.mjs")];
        assert!(
            codes(derive_path_authority(&spec, &source, &policy, &[]))
                .contains(&"path_authority_path_protected")
        );
    }

    #[test]
    fn a_prohibited_source_path_is_not_a_positive_ownership_witness() {
        let (mut spec, _, mut policy) = fixture();
        let text = "Do not modify `src/store.mjs`.";
        let source = SourceRequest::new(text.as_bytes().to_vec()).expect("source");
        let mut requirement = spec.requirements.remove(3);
        requirement.source_refs = vec![span(text, text)];
        requirement.force = NormativeForce::MustNot;
        spec.requirements = vec![requirement];
        spec.original_request_ref = source.digest();
        spec.owned_paths = vec![path("src/store.mjs")];
        spec = spec.normalized();
        policy.writable_roots.push(path("src/store.mjs"));
        assert!(
            codes(derive_path_authority(&spec, &source, &policy, &[]))
                .contains(&"path_authority_path_prohibited")
        );
    }

    #[test]
    fn persisted_provenance_must_match_the_exact_admitted_inputs() {
        let (spec, source, policy) = fixture();
        let mut authority = derive_path_authority(&spec, &source, &policy, &[]).expect("authority");
        authority.owned_paths.remove(&path("src/store.mjs"));
        assert_eq!(
            authority
                .verify_inputs(&spec, &source, &policy, &[])
                .expect_err("missing provenance")[0]
                .code,
            "path_authority_identity_mismatch"
        );
        let different = SourceRequest::new(format!("{REQUEST} ").into_bytes()).expect("source");
        assert!(
            codes(derive_path_authority(&spec, &different, &policy, &[]))
                .contains(&"request_digest_mismatch")
        );
    }
}
