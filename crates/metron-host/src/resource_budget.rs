//! External invocation-local resource assumptions, outside deterministic task identity.
use metron_inference_local::context_budget::{
    self, AllocationRequest, BudgetPolicy, DeviceBudget, ResidentBounds, ResourceScope,
};
use metron_protocol::{Digest, PrincipalId, TaskId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u8,
    profile_digest: Digest,
    device_uuid: String,
    device_budget_bytes: u64,
    external_reserved_bytes: u64,
    accounting: String,
    route: String,
    weights_upper_bytes: Option<u64>,
    recurrent_upper_bytes: Option<u64>,
    workspace_upper_bytes: Option<u64>,
    runtime_upper_bytes: Option<u64>,
    fixed_kv_pool_upper_bytes: Option<u64>,
    kv_usable_tokens_lower_bound: Option<u64>,
    headroom_bytes: u64,
    provenance: String,
}
fn read(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(16_385)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16_384 {
        return Err("resource input exceeds 16 KiB".into());
    }
    Ok(bytes)
}
pub fn policy(
    path: &Path,
    task_id: TaskId,
    principal_id: PrincipalId,
) -> Result<BudgetPolicy, String> {
    let d: Document =
        serde_json::from_slice(&read(path)?).map_err(|e| format!("resource budget: {e}"))?;
    if d.schema_version != 1
        || d.route != "paged_scalar4_upper_bound"
        || d.accounting != "disjoint_upper_bounds"
    {
        return Err("unsupported resource budget schema/route/accounting".into());
    }
    Ok(BudgetPolicy {
        pool: DeviceBudget::new(
            d.device_uuid.clone(),
            d.device_budget_bytes,
            d.external_reserved_bytes,
        )
        .map_err(|e| format!("resource budget: {e:?}"))?,
        bounds: ResidentBounds {
            scope: ResourceScope {
                profile_digest: d.profile_digest,
                device_uuid: d.device_uuid,
            },
            weights: d.weights_upper_bytes,
            recurrent: d.recurrent_upper_bytes,
            workspace: d.workspace_upper_bytes,
            runtime: d.runtime_upper_bytes,
            fixed_kv_pool_bytes: d.fixed_kv_pool_upper_bytes,
            kv_usable_tokens_lower_bound: d.kv_usable_tokens_lower_bound,
            margin: d.headroom_bytes,
            provenance: d.provenance,
            disjoint_upper_bounds: true,
        },
        task_id,
        principal_id,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    planner_min: u64,
    planner_desired: u64,
    planner_output: u64,
    executor_count: u16,
    executor_context: u64,
    executor_output: u64,
    usable_tokens: Option<u64>,
    memory: Option<Memory>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Memory {
    device_budget_bytes: u64,
    external_reserved_bytes: u64,
    weights_upper_bytes: Option<u64>,
    recurrent_upper_bytes: Option<u64>,
    workspace_upper_bytes: Option<u64>,
    runtime_upper_bytes: Option<u64>,
    headroom_bytes: u64,
    scalar_bytes: u64,
    accounting: String,
    provenance: String,
}
pub fn calculate(path: &Path) -> Result<Value, String> {
    let r: Request = serde_json::from_slice(&read(path)?).map_err(|e| e.to_string())?;
    let derived = r
        .memory
        .as_ref()
        .map(|m| {
            if m.accounting != "disjoint_upper_bounds"
                || m.provenance.is_empty()
                || m.provenance.len() > 2048
            {
                return Err(
                    "memory bounds require disjoint_upper_bounds and provenance".to_string()
                );
            }
            context_budget::usable_tokens_from_memory(
                m.device_budget_bytes,
                m.external_reserved_bytes,
                &[
                    m.weights_upper_bytes,
                    m.recurrent_upper_bytes,
                    m.workspace_upper_bytes,
                    m.runtime_upper_bytes,
                ],
                m.headroom_bytes,
                m.scalar_bytes,
            )
            .map_err(|e| format!("memory budget: {e:?}"))
        })
        .transpose()?;
    let usable_tokens = match (r.usable_tokens, derived) {
        (Some(given), Some(bound)) => given.min(bound),
        (Some(given), None) => given,
        (None, Some(bound)) => bound,
        (None, None) => return Err("requires usable_tokens or complete memory bounds".into()),
    };
    let scalar_bytes = r.memory.as_ref().map_or(2, |m| m.scalar_bytes);
    let a = context_budget::allocate(AllocationRequest {
        planner_min: r.planner_min,
        planner_desired: r.planner_desired,
        planner_output: r.planner_output,
        executor_count: r.executor_count,
        executor_context: r.executor_context,
        executor_output: r.executor_output,
        usable_tokens,
        runtime_ceiling: 8192,
    })
    .map_err(|e| format!("context allocation: {e:?}"))?;
    Ok(
        json!({"outcome":"planned", "planner_context":a.planner_context,
        "executor_context":a.executor_context,"executor_count":a.executor_count,
        "simultaneous_paged_kv_payload_bytes":a.paged_kv_bytes.checked_mul(scalar_bytes / 2).ok_or("KV payload overflow")?,
        "allocation_basis":if derived.is_some() { "declared_disjoint_memory_upper_bounds" } else { "caller_supplied_usable_tokens" },
        "effective_usable_tokens":usable_tokens,"memory_derived_usable_tokens":derived,
        "planning_kv_scalar_bytes":scalar_bytes,"planning_kv_dtype_assumption":if scalar_bytes == 2 { "BF16/F16" } else { "conservative four-byte scalar upper bound" },
        "contexts_within_runtime_ceiling":a.contexts_within_runtime_ceiling,
        "physical_fit":if derived.is_some() { "conditional_on_declared_bounds" } else { "not_evaluated" }, "runtime_supported":a.runtime_supported,
        "runtime_context_ceiling":8192,"trained_context_ceiling":262144,
        "reason":"heterogeneous role scheduling is not implemented; context planning does not qualify physical fit"}),
    )
}
