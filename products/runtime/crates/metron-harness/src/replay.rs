//! Independent behavioral replay of consumed durable inputs.
use metron_protocol::{
    domain::{AgentState, Cognition, Phase, Transition},
    store::{InboxStatus, RuntimeSnapshot, StoreError},
    types::{SqlU64, TaskId},
};

/// Rerun every consumed input using the supplied version-selected engine and compare
/// each complete transition and the resulting persisted cognition projection.
pub fn behavior<S: RuntimeSnapshot>(
    snapshot: &mut S,
    task: &TaskId,
    engine: &dyn Cognition,
) -> Result<bool, StoreError> {
    let projection = snapshot.projection(task)?;
    let mut state = AgentState {
        task_id: task.clone(),
        genesis_digest: projection.state.genesis_digest.clone(),
        max_decision_steps: projection.task.data().max_decision_steps,
        revision: SqlU64::new(0).map_err(StoreError::Validation)?,
        decision_steps: 0,
        phase: Phase::New,
        pending_intent: None,
    };
    let mut cursor = None;
    let mut first_pending = None;
    loop {
        let page = snapshot.inbox(task, cursor)?;
        for row in page.records {
            match row.status {
                InboxStatus::Consumed => {
                    if first_pending.is_some() {
                        return Ok(false);
                    }
                    let expected = match engine.transition(&state, &row.input.event) {
                        Ok(t) => t,
                        Err(_) => return Ok(false),
                    };
                    if row.transition.as_ref() != Some(&expected) {
                        return Ok(false);
                    }
                    state = match expected {
                        Transition::Decision { next, .. } | Transition::Finalization { next } => {
                            next
                        }
                    };
                }
                InboxStatus::Pending => {
                    if row.transition.is_some() {
                        return Ok(false);
                    }
                    if first_pending.is_none() {
                        first_pending = Some(row.input.id);
                    }
                }
                InboxStatus::Superseded => {
                    if row.transition.is_some() {
                        return Ok(false);
                    }
                }
            }
        }
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    let pending = snapshot.pending_inbox(task)?.map(|item| item.id);
    Ok(state == projection.state && pending == first_pending)
}
