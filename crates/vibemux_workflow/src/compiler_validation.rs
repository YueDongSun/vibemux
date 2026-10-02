//! Source-traceability validation of a TaskSpec candidate (ADR 031 §2).
//!
//! A model may propose the interpretation; this validator decides whether it
//! may be admitted. It rejects, with stable codes:
//!
//! - `invented_scope`: a requirement that cites no request span;
//! - `missing_coverage`: a request clause no requirement or exclusion covers;
//! - `dropped_prohibition`: a prohibition no binding requirement covers;
//! - `weakened_force`: a binding clause rendered as `should` or `may`;
//! - `changed_number` / `invented_number`: numbers dropped from, or added
//!   to, the cited sources;
//! - `literal_not_preserved` / `literal_not_in_source`: identifiers, paths,
//!   and quoted data not carried byte for byte, or transformed;
//! - `non_english_statement`: CJK text in an instruction instead of in a
//!   literal;
//! - `span_mismatch`, `contract_excerpt_mismatch`, `request_digest_mismatch`,
//!   `prohibition_excluded`, `binding_clause_excluded`,
//!   `normative_assumption`.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    Sha256Digest,
    source_request::{
        ByteRange, SourceRequest, contains_cjk, has_binding_marker, literal_tokens, numeric_tokens,
    },
    task_spec::{ArtifactRef, NormativeForce, Requirement, TaskSpec},
};

/// The text of one shared contract artifact, supplied by the daemon from
/// the integrator-owned fixture; its digest must match the reference.
#[derive(Clone, Copy, Debug)]
pub struct SharedContractText<'a> {
    pub reference: &'a ArtifactRef,
    pub text: &'a str,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CoverageViolation {
    pub code: &'static str,
    pub requirement_id: Option<String>,
    /// Byte offset of the uncovered clause or marker, when relevant.
    pub source_offset: Option<usize>,
    /// The number or literal concerned, for the operator's semantic diff.
    /// Never copied into canonical events.
    pub detail: Option<String>,
}

impl CoverageViolation {
    fn new(code: &'static str) -> Self {
        Self {
            code,
            requirement_id: None,
            source_offset: None,
            detail: None,
        }
    }

    fn requirement(mut self, requirement: &Requirement) -> Self {
        self.requirement_id = Some(requirement.requirement_id.to_string());
        self
    }

    fn offset(mut self, offset: usize) -> Self {
        self.source_offset = Some(offset);
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Counts describing an accepted candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageReport {
    pub clause_count: usize,
    pub excluded_clause_count: usize,
    pub requirement_count: usize,
    pub prohibition_count: usize,
}

/// Validates `spec` (structurally valid and normalized) against the request
/// and the shared contract texts.
pub fn validate_coverage(
    spec: &TaskSpec,
    request: &SourceRequest,
    contracts: &[SharedContractText<'_>],
) -> Result<CoverageReport, Vec<CoverageViolation>> {
    let mut violations = Vec::new();
    if spec.original_request_ref != request.digest() {
        violations.push(CoverageViolation::new("request_digest_mismatch"));
        return Err(violations);
    }
    let contracts = verified_contracts(spec, contracts, &mut violations);
    let mut requirement_ranges: Vec<(ByteRange, NormativeForce)> = Vec::new();
    for requirement in &spec.requirements {
        let ranges = validate_requirement(requirement, request, &contracts, &mut violations);
        requirement_ranges.extend(ranges.into_iter().map(|range| (range, requirement.force)));
    }
    let mut exclusion_ranges = Vec::new();
    for exclusion in &spec.source_exclusions {
        let Some(range) = request.resolve(&exclusion.span) else {
            violations.push(CoverageViolation::new("span_mismatch").detail("source_exclusions"));
            continue;
        };
        let text = &request.text()[range.start..range.end];
        if request
            .prohibition_markers()
            .iter()
            .any(|marker| range.overlaps(*marker))
        {
            violations.push(CoverageViolation::new("prohibition_excluded").offset(range.start));
        } else if has_binding_marker(text) {
            violations.push(CoverageViolation::new("binding_clause_excluded").offset(range.start));
        }
        exclusion_ranges.push(range);
    }
    let clauses = request.clauses();
    let mut excluded_clause_count = 0;
    for clause in &clauses {
        let covered = requirement_ranges
            .iter()
            .any(|(range, _)| range.overlaps(*clause));
        let excluded = exclusion_ranges.iter().any(|range| range.overlaps(*clause));
        if !covered && excluded {
            excluded_clause_count += 1;
        }
        if !covered && !excluded {
            violations.push(CoverageViolation::new("missing_coverage").offset(clause.start));
        }
    }
    let prohibitions = request.prohibition_markers();
    for marker in &prohibitions {
        let bound = requirement_ranges
            .iter()
            .any(|(range, force)| force.is_binding() && range.contains(*marker));
        if !bound {
            violations.push(CoverageViolation::new("dropped_prohibition").offset(marker.start));
        }
    }
    for assumption in &spec.assumptions {
        if has_binding_marker(&assumption.statement) {
            violations.push(
                CoverageViolation::new("normative_assumption")
                    .detail(assumption.assumption_id.to_string()),
            );
        }
    }
    if violations.is_empty() {
        Ok(CoverageReport {
            clause_count: clauses.len(),
            excluded_clause_count,
            requirement_count: spec.requirements.len(),
            prohibition_count: prohibitions.len(),
        })
    } else {
        violations.sort();
        violations.dedup();
        Err(violations)
    }
}

/// Contract texts whose digest matches a reference the spec declares.
fn verified_contracts<'a>(
    spec: &TaskSpec,
    contracts: &[SharedContractText<'a>],
    violations: &mut Vec<CoverageViolation>,
) -> Vec<SharedContractText<'a>> {
    let mut verified = Vec::new();
    for contract in contracts {
        let declared = spec.shared_contract_refs.contains(contract.reference);
        let digest_matches = Sha256Digest::of(contract.text.as_bytes())
            == contract.reference.sha256
            && contract.text.len() as u64 == contract.reference.byte_count;
        if declared && digest_matches {
            verified.push(*contract);
        } else if declared {
            violations.push(
                CoverageViolation::new("contract_excerpt_mismatch")
                    .detail(contract.reference.artifact_id.to_string()),
            );
        }
    }
    verified
}

/// Checks one requirement and returns the request ranges it cites.
fn validate_requirement(
    requirement: &Requirement,
    request: &SourceRequest,
    contracts: &[SharedContractText<'_>],
    violations: &mut Vec<CoverageViolation>,
) -> Vec<ByteRange> {
    let mut ranges = Vec::new();
    for span in &requirement.source_refs {
        match request.resolve(span) {
            Some(range) => ranges.push(range),
            None => {
                violations.push(CoverageViolation::new("span_mismatch").requirement(requirement))
            }
        }
    }
    if requirement.source_refs.is_empty() {
        violations.push(CoverageViolation::new("invented_scope").requirement(requirement));
    }
    let mut excerpt_texts = Vec::new();
    for excerpt in &requirement.contract_excerpts {
        let found = contracts.iter().any(|contract| {
            contract.reference.artifact_id == excerpt.artifact_id
                && !excerpt.quoted.is_empty()
                && contract.text.contains(excerpt.quoted.as_str())
        });
        if found {
            excerpt_texts.push(excerpt.quoted.as_str());
        } else {
            violations
                .push(CoverageViolation::new("contract_excerpt_mismatch").requirement(requirement));
        }
    }
    if contains_cjk(&requirement.statement) {
        violations.push(CoverageViolation::new("non_english_statement").requirement(requirement));
    }
    let span_texts: Vec<&str> = ranges
        .iter()
        .map(|range| &request.text()[range.start..range.end])
        .collect();

    // Numbers: everything the request spans state must survive, and nothing
    // may appear that neither the spans nor the contract excerpts state.
    let requirement_numbers: BTreeSet<String> = numeric_tokens(&requirement.statement)
        .into_iter()
        .chain(
            requirement
                .literals
                .iter()
                .flat_map(|literal| numeric_tokens(literal)),
        )
        .collect();
    let request_numbers: BTreeSet<String> = span_texts
        .iter()
        .flat_map(|text| numeric_tokens(text))
        .collect();
    let source_numbers: BTreeSet<String> = request_numbers
        .iter()
        .cloned()
        .chain(excerpt_texts.iter().flat_map(|text| numeric_tokens(text)))
        .collect();
    for number in request_numbers.difference(&requirement_numbers) {
        violations.push(
            CoverageViolation::new("changed_number")
                .requirement(requirement)
                .detail(number.clone()),
        );
    }
    for number in requirement_numbers.difference(&source_numbers) {
        violations.push(
            CoverageViolation::new("invented_number")
                .requirement(requirement)
                .detail(number.clone()),
        );
    }

    // Literals: carried byte for byte in both directions.
    let literals: BTreeSet<&str> = requirement.literals.iter().map(String::as_str).collect();
    for range in &ranges {
        for token in literal_tokens(request.text(), *range) {
            if !literals.contains(token.as_str()) {
                violations.push(
                    CoverageViolation::new("literal_not_preserved")
                        .requirement(requirement)
                        .detail(token),
                );
            }
        }
    }
    for literal in &requirement.literals {
        let in_source = span_texts
            .iter()
            .chain(excerpt_texts.iter())
            .any(|text| text.contains(literal.as_str()));
        if !in_source {
            violations.push(
                CoverageViolation::new("literal_not_in_source")
                    .requirement(requirement)
                    .detail(literal.clone()),
            );
        }
    }

    // Force: a binding clause cannot become optional.
    let binding_source = span_texts.iter().any(|text| has_binding_marker(text));
    if binding_source && !requirement.force.is_binding() {
        violations.push(CoverageViolation::new("weakened_force").requirement(requirement));
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_spec::{
        ContractExcerpt, ExclusionReason, MediaType, SourceExclusion, fixtures::*,
    };

    fn request(text: &str) -> SourceRequest {
        SourceRequest::new(text.as_bytes().to_vec()).expect("valid request")
    }

    fn codes(result: Result<CoverageReport, Vec<CoverageViolation>>) -> Vec<&'static str> {
        result
            .expect_err("violations expected")
            .into_iter()
            .map(|violation| violation.code)
            .collect()
    }

    #[test]
    fn faithful_candidate_is_accepted() {
        let spec = track_a_spec().normalized();
        let report = validate_coverage(&spec, &request(REQUEST), &[]).expect("accepted");
        assert_eq!(report.clause_count, 4);
        assert_eq!(report.prohibition_count, 1);
    }

    #[test]
    fn dropped_prohibition_is_rejected() {
        let mut spec = track_a_spec();
        spec.requirements
            .retain(|requirement| requirement.requirement_id.as_str() != "r03");
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &[],
        ));
        assert!(found.contains(&"dropped_prohibition"));
        assert!(found.contains(&"missing_coverage"));
    }

    #[test]
    fn weakened_prohibition_is_rejected() {
        let mut spec = track_a_spec();
        spec.requirements[2].force = NormativeForce::Should;
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &[],
        ));
        assert!(found.contains(&"dropped_prohibition"));
        assert!(found.contains(&"weakened_force"));
    }

    #[test]
    fn changed_number_is_rejected_both_ways() {
        let mut spec = track_a_spec();
        spec.requirements[1].statement = "Reject titles longer than 100 characters.".into();
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &[],
        ));
        assert!(found.contains(&"changed_number"));
        assert!(found.contains(&"invented_number"));
    }

    #[test]
    fn invented_scope_is_rejected() {
        let mut spec = track_a_spec();
        let mut invented = spec.requirements[0].clone();
        invented.requirement_id = id("r09");
        invented.statement = "Add user authentication.".into();
        invented.source_refs.clear();
        spec.requirements.push(invented);
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &[],
        ));
        assert_eq!(found, vec!["invented_scope"]);
    }

    #[test]
    fn transformed_or_dropped_literals_are_rejected() {
        let mut spec = track_a_spec();
        spec.requirements[3].literals = vec!["src/Store.mjs".into()];
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &[],
        ));
        assert!(found.contains(&"literal_not_preserved"));
        assert!(found.contains(&"literal_not_in_source"));
    }

    #[test]
    fn unicode_literals_must_stay_literals_not_instructions() {
        let text = "Show the title “任务 ✓” exactly.";
        let source = request(text);
        let mut spec = track_a_spec();
        spec.original_request_ref = source.digest();
        spec.requirements.truncate(1);
        spec.requirements[0].source_refs = vec![span(text, text)];
        spec.requirements[0].statement = "Render the title 任务 ✓ exactly.".into();
        let found = codes(validate_coverage(&spec.clone().normalized(), &source, &[]));
        assert!(found.contains(&"non_english_statement"));
        assert!(found.contains(&"literal_not_preserved"));
        spec.requirements[0].statement = "Render the quoted title exactly.".into();
        spec.requirements[0].literals = vec!["任务 ✓".into()];
        validate_coverage(&spec.normalized(), &source, &[]).expect("literal carried as data");
    }

    #[test]
    fn exclusions_cannot_hide_obligations_or_prohibitions() {
        let text = "I like boards. Never store passwords.";
        let source = request(text);
        let mut spec = track_a_spec();
        spec.original_request_ref = source.digest();
        spec.requirements.truncate(1);
        spec.requirements[0].source_refs = vec![span(text, "I like boards.")];
        spec.source_exclusions = vec![SourceExclusion {
            span: span(text, "Never store passwords."),
            reason: ExclusionReason::BackgroundContext,
        }];
        let found = codes(validate_coverage(&spec.clone().normalized(), &source, &[]));
        assert!(found.contains(&"prohibition_excluded"));
        assert!(found.contains(&"dropped_prohibition"));
        // Background may be excluded when the obligation is covered.
        spec.requirements[0].source_refs = vec![span(text, "Never store passwords.")];
        spec.requirements[0].force = NormativeForce::MustNot;
        spec.requirements[0].statement = "Do not store passwords.".into();
        spec.source_exclusions = vec![SourceExclusion {
            span: span(text, "I like boards."),
            reason: ExclusionReason::BackgroundContext,
        }];
        let report = validate_coverage(&spec.normalized(), &source, &[]).expect("covered");
        assert_eq!(report.excluded_clause_count, 1);
    }

    #[test]
    fn numbers_may_come_from_verified_contract_excerpts_only() {
        let contract = "Bodies larger than 8192 bytes return 413 payload_too_large.";
        let reference = ArtifactRef {
            artifact_id: id("contract_md"),
            sha256: Sha256Digest::of(contract.as_bytes()),
            byte_count: contract.len() as u64,
            media_type: MediaType::TextMarkdown,
        };
        let mut spec = track_a_spec();
        spec.shared_contract_refs = vec![reference.clone()];
        spec.requirements[0].statement =
            "Implement the backend; oversized bodies return 413.".into();
        spec.requirements[0].contract_excerpts = vec![ContractExcerpt {
            artifact_id: id("contract_md"),
            quoted: "return 413 payload_too_large".into(),
        }];
        let texts = [SharedContractText {
            reference: &reference,
            text: contract,
        }];
        validate_coverage(&spec.clone().normalized(), &request(REQUEST), &texts).expect("traced");
        // Without the verified contract text the number is invented.
        let found = codes(validate_coverage(
            &spec.clone().normalized(),
            &request(REQUEST),
            &[],
        ));
        assert!(found.contains(&"invented_number"));
        assert!(found.contains(&"contract_excerpt_mismatch"));
        // A tampered contract text does not verify.
        let tampered = [SharedContractText {
            reference: &reference,
            text: "return 413 payload_too_large",
        }];
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &tampered,
        ));
        assert!(found.contains(&"contract_excerpt_mismatch"));
    }

    #[test]
    fn a_different_request_is_rejected_up_front() {
        let found = codes(validate_coverage(
            &track_a_spec().normalized(),
            &request("Build something else."),
            &[],
        ));
        assert_eq!(found, vec!["request_digest_mismatch"]);
    }

    #[test]
    fn normative_assumptions_are_rejected() {
        let mut spec = track_a_spec();
        spec.assumptions[0].statement = "The server must also expose metrics.".into();
        let found = codes(validate_coverage(
            &spec.normalized(),
            &request(REQUEST),
            &[],
        ));
        assert_eq!(found, vec!["normative_assumption"]);
    }
}
