//! Non-deterministic model interpretation, followed by the existing deterministic admission.
use std::sync::Arc;

use time::OffsetDateTime;
use tokio::sync::watch;
use uuid::Uuid;
use vibemux_workflow::{
    Sha256Digest, SpecIdentifier,
    gateway::{GatewayConfig, RouteRole},
    model_call::{ModelCallAdmission, ModelCallRequest, compile_task_candidates},
    planning_templates::PlanningTemplateSet,
    policy::OperatorPolicy,
};

use super::{
    error::WorkflowError,
    model_runtime::{ModelGateway, run_model_call},
    request::WorkflowRequest,
    runtime::WorkflowSetup,
};
use crate::{
    WriterHandle,
    harness_dispatch::{call_writer, git_head::read_head_commit},
};

const COMPILER_REQUEST_DOMAIN: &str = "vibemux.workflow.compiler_request.v1";

/// Supplied by an authorized startup owner. A model callback cannot install a factory.
pub trait WorkflowModelGatewayFactory: Send + Sync {
    fn for_workflow(
        &self,
        workflow_id: Uuid,
        config: &GatewayConfig,
        config_digest: Sha256Digest,
    ) -> Result<Arc<dyn ModelGateway>, WorkflowError>;
}

pub(crate) async fn compile_missing_specs(
    writer: &WriterHandle,
    setup: &WorkflowSetup,
    factory: Option<&Arc<dyn WorkflowModelGatewayFactory>>,
    request: &mut WorkflowRequest,
    policy: &OperatorPolicy,
) -> Result<(), WorkflowError> {
    if !request.task_specs.is_empty() {
        return Ok(());
    }
    let workflow_id = super::turns::workflow_id_for(&request.request_key);
    let lookup = writer.clone();
    let existing = call_writer(move || lookup.workflow_record(workflow_id)).await?;
    if let Some(record) = existing {
        let state = setup.state.clone();
        let plan = tokio::task::spawn_blocking(move || state.read_plan(workflow_id))
            .await
            .map_err(|_| WorkflowError::Internal)??;
        if record.policy_digest != policy.digest().map_err(|_| WorkflowError::PolicyInvalid)?
            || plan.request_sha256 != Sha256Digest::of(request.request_text.as_bytes())
            || plan.assignments != request.assignments
            || plan.reviewer_slot != request.reviewer_slot
        {
            return Err(WorkflowError::RequestConflict);
        }
        request.task_specs = plan
            .specs
            .values()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|_| WorkflowError::Internal)?;
        return Ok(());
    }
    let config = setup
        .config
        .config
        .gateway
        .as_ref()
        .ok_or(WorkflowError::CompilerUnavailable)?;
    let binding = config
        .binding(RouteRole::Compiler)
        .ok_or(WorkflowError::CompilerUnavailable)?;
    let factory = factory.ok_or_else(|| WorkflowError::Dispatch {
        code: "gateway_live_attestation_unavailable".into(),
    })?;
    let gateway = factory.for_workflow(workflow_id, config, setup.config.digest)?;
    let mut input = serde_json::to_value(&*request).map_err(|_| WorkflowError::RequestInvalid)?;
    let root = setup.canonical_root.clone();
    let base = tokio::task::spawn_blocking(move || read_head_commit(&root))
        .await
        .map_err(|_| WorkflowError::Internal)?
        .map_err(|_| WorkflowError::Git)?;
    input["base_commit"] = serde_json::json!(base);
    input["original_request_ref"] =
        serde_json::json!(Sha256Digest::of(request.request_text.as_bytes()));
    input["operator_policy"] =
        serde_json::to_value(policy).map_err(|_| WorkflowError::PolicyInvalid)?;
    // An absent template keeps the builtin compiler instruction; a present
    // but unknown name fails closed.
    let template = request
        .compiler_template
        .as_ref()
        .map_or(Some(PlanningTemplateSet::BuiltinV1), |name| {
            PlanningTemplateSet::parse(name.as_str())
        })
        .ok_or(WorkflowError::RequestInvalid)?;
    let model_request = ModelCallRequest {
        role: RouteRole::Compiler,
        requested_alias: binding.model_alias.clone(),
        protocol: binding.protocol,
        max_output_tokens: binding.max_output_tokens,
        timeout_ms: binding.timeout_ms,
        config_sha256: setup.config.digest,
        observation_digest: None,
        system_prompt: template.compiler_system().into(),
        user_prompt: serde_json::to_string(&input).map_err(|_| WorkflowError::RequestInvalid)?,
    };
    let identity = model_request
        .identity()
        .map_err(|_| WorkflowError::RequestInvalid)?;
    let digest = Sha256Digest::of_fields(
        COMPILER_REQUEST_DOMAIN,
        &[
            workflow_id.as_bytes(),
            identity.request_fingerprint.as_bytes(),
        ],
    );
    let call_id = Uuid::from_bytes(
        digest.as_bytes()[..16]
            .try_into()
            .map_err(|_| WorkflowError::Internal)?,
    );
    let now = super::runtime::millis(OffsetDateTime::now_utc());
    // A compiler makes at most one interpretation call under this request key.
    // A repeated prepared request reuses its admitted plan; ambiguous calls refuse replay.
    let mut admission = ModelCallAdmission {
        call_id,
        owner_key: SpecIdentifier::new(format!("compiler_{}", workflow_id.simple()))
            .map_err(|_| WorkflowError::Internal)?,
        workflow_id: None,
        identity,
        deadline_unix_ms: now
            .checked_add(binding.timeout_ms)
            .ok_or(WorkflowError::PolicyInvalid)?,
        max_calls: 1,
    };
    let lookup = writer.clone();
    if let Some(prior) = call_writer(move || lookup.model_call(call_id)).await? {
        admission.deadline_unix_ms = prior.admission.deadline_unix_ms;
    }
    let (_cancel, cancellation) = watch::channel(false);
    let output = run_model_call(
        writer,
        gateway.as_ref(),
        model_request,
        admission,
        cancellation,
    )
    .await
    .map_err(|error| WorkflowError::Dispatch {
        code: error.code().into(),
    })?;
    request.task_specs = compile_task_candidates(&output.text)
        .map_err(|error| WorkflowError::ContractRejected {
            codes: vec![error.code().into()],
        })?
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()
        .map_err(|_| WorkflowError::Internal)?;
    Ok(())
}
