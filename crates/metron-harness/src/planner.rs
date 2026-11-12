//! Pure coding planning through the shared versioned controller.
use metron_protocol::{AgentEvent, AgentState, Cognition, DecisionError, Transition};

/// Pure version-3 coding controller. Model and tool data remain proposals.
#[derive(Clone, Debug)]
pub struct CodingPlanner {
    task: metron_protocol::coding::CodingTask,
    policy: metron_protocol::coding::CodingPolicy,
    version: u8,
}
impl CodingPlanner {
    pub const ENGINE_VERSION: &str = "coding-harness/v3.10";
    pub const CONSTRAINED_ENGINE_VERSION: &str = "coding-harness/v3.6";
    pub const RECOVERING_ENGINE_VERSION: &str = "coding-harness/v3.5";
    pub const NATIVE_ENGINE_VERSION: &str = "coding-harness/v3.4";
    pub const FOCUSED_ENGINE_VERSION: &str = "coding-harness/v3.3";
    pub const CATALOG_ENGINE_VERSION: &str = "coding-harness/v3.2";
    pub const PHASED_ENGINE_VERSION: &str = "coding-harness/v3.1";
    pub const LEGACY_ENGINE_VERSION: &str = "coding-harness/v3";
    pub fn new(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 10,
        })
    }
    pub fn legacy_v3_9(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 9,
        })
    }
    pub fn legacy_v3_8(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 8,
        })
    }
    pub fn legacy_v3_7(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 7,
        })
    }
    pub fn legacy_v3_6(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 6,
        })
    }
    pub fn legacy_v3_5(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 5,
        })
    }
    pub fn legacy_v3_4(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 4,
        })
    }
    pub fn legacy_v3_3(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 3,
        })
    }
    pub fn legacy_v3(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 0,
        })
    }
    pub fn legacy_v3_1(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 1,
        })
    }
    pub fn legacy_v3_2(
        task: &metron_protocol::coding::CodingTask,
        policy: &metron_protocol::coding::CodingPolicy,
    ) -> Result<Self, metron_protocol::ValidationError> {
        policy.validate(task)?;
        Ok(Self {
            task: task.clone(),
            policy: policy.clone(),
            version: 2,
        })
    }
}
impl Cognition for CodingPlanner {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError> {
        match self.version {
            0 => metron_protocol::coding_runtime::coding_transition(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            1 => metron_protocol::coding_runtime::coding_transition_phased(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            2 => metron_protocol::coding_runtime::coding_transition_catalog(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            3 => metron_protocol::coding_runtime::coding_transition_focused(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            4 => metron_protocol::coding_runtime::coding_transition_native(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            5 => metron_protocol::coding_runtime::coding_transition_recovering(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            6 => metron_protocol::coding_runtime::coding_transition_constrained(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            7 => metron_protocol::coding_runtime::coding_transition_format_neutral(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            10 => metron_protocol::coding_runtime::coding_transition_detailed_feedback(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            9 => metron_protocol::coding_runtime::coding_transition_paired(
                &self.task,
                &self.policy,
                state,
                event,
            ),
            _ => metron_protocol::coding_runtime::coding_transition_memory(
                &self.task,
                &self.policy,
                state,
                event,
            ),
        }
    }
}
