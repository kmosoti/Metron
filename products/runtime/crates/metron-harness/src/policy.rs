//! Admission authority is local to this module and is consumed by an effect driver.
use metron_protocol::{
    domain::{ApprovedRequest, PreparedIntent},
    store::{
        AdmissionCheck, ApprovalDecision, Disposition, Genesis, GrantSnapshot as StoredGrant,
        IntentSnapshot, Projection, StoreCommand, StoreError, StoreOperation, StorePort,
        StoreResult, WriteOutcome,
    },
    types::content_digest,
};

// These wrappers are process-local policy evidence, never serialized or cloned.
struct HostPolicy {
    publication_approval: bool,
}
impl HostPolicy {
    fn from_genesis(genesis: &Genesis) -> Result<Self, StoreError> {
        if genesis.policy_digest != content_digest(b"metron-alpha0-host-policy-v1") {
            return Err(StoreError::Integrity);
        }
        Ok(Self {
            publication_approval: genesis.require_publication_approval,
        })
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
        if policy.publication_approval
            && matches!(
                check.prepared.request,
                ApprovedRequest::PublishArtifact { .. }
            )
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
        Ok(Self {
            check,
            revoked: grant.revoked,
        })
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
    let policy = HostPolicy::from_genesis(&projection.genesis)?;
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
