//! Admission authority is local to this module and is consumed by an effect driver.
use metron_protocol::{
    domain::{ApprovedRequest, PreparedIntent},
    store::{
        AdmissionCheck, ApprovalDecision, Disposition, GrantSnapshot as StoredGrant,
        IntentSnapshot, Projection, StoreCommand, StoreError, StoreOperation, StorePort,
        StoreResult, WriteOutcome,
    },
    types::content_digest,
};

// These wrappers are process-local policy evidence, never serialized or cloned.
struct HostPolicy {
    publication_approval: bool,
    coding: Option<metron_protocol::coding::CodingPolicy>,
}
impl HostPolicy {
    fn from_projection(projection: &Projection) -> Result<Self, StoreError> {
        let genesis = &projection.genesis;
        let coding = if let Some(task) = projection.task.coding() {
            let policy = genesis
                .coding_policy
                .as_deref()
                .ok_or(StoreError::Integrity)?;
            if policy.digest(task).map_err(StoreError::Validation)? != genesis.policy_digest {
                return Err(StoreError::Integrity);
            }
            Some(policy.clone())
        } else {
            if genesis.coding_policy.is_some()
                || genesis.policy_digest != content_digest(b"metron-alpha0-host-policy-v1")
            {
                return Err(StoreError::Integrity);
            }
            None
        };
        Ok(Self {
            publication_approval: genesis.require_publication_approval,
            coding,
        })
    }
    fn requires_approval(&self, request: &ApprovedRequest) -> bool {
        match request {
            ApprovedRequest::Coding { step, .. } => self
                .coding
                .as_ref()
                .is_some_and(|p| step.requires_approval(p)),
            ApprovedRequest::PublishArtifact { .. } => self.publication_approval,
            _ => false,
        }
    }
    fn permits(&self, request: &ApprovedRequest) -> bool {
        match request {
            ApprovedRequest::Coding { step, .. } => {
                self.coding.as_ref().is_some_and(|p| step.permitted(p))
            }
            _ => self.coding.is_none(),
        }
    }
}
struct GrantSnapshot {
    revision: metron_protocol::SqlU64,
    revoked: bool,
}
impl GrantSnapshot {
    fn checked(grant: &StoredGrant, projection: &Projection) -> Result<Self, StoreError> {
        if grant.principal_id != projection.task.data().principal_id {
            return Err(StoreError::Integrity);
        }
        Ok(Self {
            revision: grant.revision,
            revoked: grant.revoked,
        })
    }
}
struct DispatchAdmission {
    check: AdmissionCheck,
    revoked: bool,
}
impl DispatchAdmission {
    fn checked(
        check: AdmissionCheck,
        intent: &IntentSnapshot,
        projection: &Projection,
        grant: GrantSnapshot,
        policy: &HostPolicy,
    ) -> Result<Self, StoreError> {
        if check.intent_id != intent.intent_id
            || check.prepared != *intent.prepared.as_ref().ok_or(StoreError::Integrity)?
            || check.grant_revision != grant.revision
            || check.expected_tick != projection.tick
        {
            return Err(StoreError::Integrity);
        }
        check
            .prepared
            .validate_against_genesis(
                &intent.intent,
                &projection.task.data().principal_id,
                &projection.state.genesis_digest,
                &projection.genesis,
            )
            .map_err(StoreError::Validation)?;
        if policy.permits(&check.prepared.request)
            && policy.requires_approval(&check.prepared.request)
        {
            let approval = intent.approval.as_ref().ok_or(StoreError::Integrity)?;
            if check.approval_id.as_ref() != Some(&approval.request_id)
                || approval.action_digest != check.prepared.action_digest
                || approval.expires_at <= projection.tick
                || approval.decision != Some(ApprovalDecision::Approve)
            {
                return Err(StoreError::Integrity);
            }
        }
        let revoked = grant.revoked || !policy.permits(&check.prepared.request);
        Ok(Self { check, revoked })
    }
}

/// A single admitted effect. It cannot be constructed, cloned or deserialized by callers.
///
/// ```compile_fail
/// use metron_harness::policy::AuthorizedEffect;
/// fn forge(prepared: metron_protocol::PreparedIntent) -> AuthorizedEffect {
///     AuthorizedEffect { prepared }
/// }
/// ```
///
/// ```compile_fail
/// use metron_harness::policy::AuthorizedEffect;
/// let _: AuthorizedEffect = metron_protocol::parse_envelope(b"{}").unwrap();
/// ```
///
/// ```compile_fail
/// use metron_harness::policy::AuthorizedEffect;
/// let _ = AuthorizedEffect::default();
/// ```
///
/// ```compile_fail
/// use metron_harness::policy::AuthorizedEffect;
/// fn duplicate(effect: &AuthorizedEffect) -> AuthorizedEffect { effect.clone() }
/// ```
pub struct AuthorizedEffect {
    prepared: PreparedIntent,
    schema_version: metron_protocol::SchemaVersion,
}
impl AuthorizedEffect {
    pub fn request(&self) -> &PreparedIntent {
        &self.prepared
    }
    pub fn schema_version(&self) -> metron_protocol::SchemaVersion {
        self.schema_version
    }
}

/// Drivers consume admission exactly once.
///
/// ```
/// use metron_harness::policy::{AuthorizedEffect, EffectDriver};
/// struct Driver;
/// impl EffectDriver for Driver {
///     type Outcome = ();
///     fn execute(&mut self, effect: AuthorizedEffect) {
///         let _ = &effect.request().request;
///     }
/// }
/// ```
///
/// ```compile_fail
/// use metron_harness::policy::{AuthorizedEffect, EffectDriver};
/// fn twice<D: EffectDriver>(driver: &mut D, effect: AuthorizedEffect) {
///     driver.execute(effect);
///     driver.execute(effect);
/// }
/// ```
pub trait EffectDriver {
    type Outcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> Self::Outcome;
}

pub(crate) enum AdmissionOutcome {
    Authorized(Box<AuthorizedEffect>),
    Denied,
    Blocked,
}

/// A retry after an ambiguous acknowledgement must set `ambiguous` and cannot mint.
pub(crate) fn admit<S: StorePort>(
    store: &mut S,
    command: StoreCommand,
    ambiguous: bool,
    projection: &Projection,
    intent: &IntentSnapshot,
    grant: &StoredGrant,
) -> Result<AdmissionOutcome, StoreError> {
    let check: AdmissionCheck = match &command.data().operation {
        StoreOperation::Admit { check } => check.clone(),
        _ => return Err(StoreError::Integrity),
    };
    let policy = HostPolicy::from_projection(projection)?;
    let grant = GrantSnapshot::checked(grant, projection)?;
    let admission = DispatchAdmission::checked(check, intent, projection, grant, &policy)?;
    let outcome = match store.apply(command.clone()) {
        Ok(outcome) => outcome,
        Err(error @ StoreError::Operational(_)) => {
            // The acknowledgement is ambiguous. Recovery is never an authority source.
            // An identical retry records a dispatch if the first write did not land.
            return match store.apply(command) {
                Ok(_) => Err(error),
                Err(recovery_error) => Err(recovery_error),
            };
        }
        Err(error) => return Err(error),
    };
    match outcome {
        WriteOutcome::Committed(reply)
            if !ambiguous && reply.disposition == Disposition::FirstCommit =>
        {
            match reply.result {
                StoreResult::Admitted { dispatch_id }
                    if !admission.revoked
                        && dispatch_id == admission.check.prepared.action.dispatch_id =>
                {
                    Ok(AdmissionOutcome::Authorized(Box::new(AuthorizedEffect {
                        prepared: admission.check.prepared,
                        schema_version: projection.task.data().schema_version,
                    })))
                }
                StoreResult::Denied => Ok(AdmissionOutcome::Denied),
                _ => Err(StoreError::Integrity),
            }
        }
        WriteOutcome::Committed(reply) if matches!(reply.result, StoreResult::Denied) => {
            Ok(AdmissionOutcome::Denied)
        }
        _ => Ok(AdmissionOutcome::Blocked),
    }
}
