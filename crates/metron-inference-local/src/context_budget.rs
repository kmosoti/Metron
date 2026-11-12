//! Conditional, host-local admission. Declared upper bounds are not physical qualification.
use crate::resources::{TRAINED_CONTEXT_TOKENS, lfm3_paged_geometry};
use metron_protocol::{Digest, LocalDevice, LocalProfile, PrincipalId, TaskId};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceScope {
    pub profile_digest: Digest,
    pub device_uuid: String,
}

/// All bounds must be disjoint, include load peaks, and cover the full worker.
/// `None` is unknown. Inclusive process observations are not component bounds.
#[derive(Clone, Debug)]
pub struct ResidentBounds {
    pub scope: ResourceScope,
    pub weights: Option<u64>,
    pub recurrent: Option<u64>,
    pub workspace: Option<u64>,
    pub runtime: Option<u64>,
    pub fixed_kv_pool_bytes: Option<u64>,
    pub kv_usable_tokens_lower_bound: Option<u64>,
    pub margin: u64,
    pub provenance: String,
    pub disjoint_upper_bounds: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetError {
    Invalid,
    Overflow,
    Unqualified,
    Scope,
    Capacity,
    Poisoned,
}

/// Round each independent demand, then add the shared pool's reserved block once.
pub fn paged_pool_bytes(demands: &[u64]) -> Result<u64, BudgetError> {
    pool_bytes(demands, 2)
}

fn pool_bytes(demands: &[u64], scalar_bytes: u64) -> Result<u64, BudgetError> {
    if demands.is_empty() || demands.len() > 16 || demands.contains(&0) {
        return Err(BudgetError::Invalid);
    }
    let tokens = demands
        .iter()
        .try_fold(0_u64, |sum, tokens| {
            tokens
                .div_ceil(32)
                .checked_mul(32)
                .and_then(|n| sum.checked_add(n))
        })
        .ok_or(BudgetError::Overflow)?;
    lfm3_paged_geometry(tokens, 32, scalar_bytes)
        .map(|g| g.tensor_payload_bytes)
        .map_err(|_| BudgetError::Overflow)
}

pub fn resident_bytes(profile: &LocalProfile, bounds: &ResidentBounds) -> Result<u64, BudgetError> {
    profile.validate().map_err(|_| BudgetError::Invalid)?;
    let LocalDevice::Cuda { uuid, .. } = &profile.device else {
        return Err(BudgetError::Scope);
    };
    if bounds.scope.profile_digest != profile.digest().map_err(|_| BudgetError::Invalid)?
        || bounds.scope.device_uuid != uuid.as_str()
    {
        return Err(BudgetError::Scope);
    }
    // Only this pinned paged route has a source-derived allocation formula.
    if !profile.chunked_prefill || crate::chunk_prefill::kv_target(profile).is_err() {
        return Err(BudgetError::Unqualified);
    }
    if !bounds.disjoint_upper_bounds
        || bounds.provenance.is_empty()
        || bounds.provenance.len() > 2048
    {
        return Err(BudgetError::Unqualified);
    }
    // Auto cache dtype is not profile-pinned. Cover F16/BF16/F32 using four bytes.
    let required = pool_bytes(
        &vec![u64::from(profile.context_tokens); usize::from(profile.max_sequences)],
        4,
    )?;
    let usable_lower = bounds
        .kv_usable_tokens_lower_bound
        .ok_or(BudgetError::Unqualified)?;
    let required_tokens = u64::from(profile.context_tokens)
        .div_ceil(32)
        .checked_mul(32)
        .and_then(|n| n.checked_mul(u64::from(profile.max_sequences)))
        .ok_or(BudgetError::Overflow)?;
    if usable_lower < required_tokens {
        return Err(BudgetError::Capacity);
    }
    let fixed = bounds.fixed_kv_pool_bytes.ok_or(BudgetError::Unqualified)?;
    if pool_bytes(&[usable_lower], 4)? > fixed
        || fixed < required
        || profile.cache_target_bytes.is_some_and(|n| fixed < n.get())
        || profile
            .cache_target_bytes
            .is_none_or(|n| n.get() < required)
    {
        return Err(BudgetError::Capacity);
    }
    // Charge the complete declared fixed pool, including reserved block and any excess.
    [
        Some(fixed),
        bounds.weights,
        bounds.recurrent,
        bounds.workspace,
        bounds.runtime,
        Some(bounds.margin),
    ]
    .into_iter()
    .try_fold(0_u64, |sum, n| {
        sum.checked_add(n.ok_or(BudgetError::Unqualified)?)
            .ok_or(BudgetError::Overflow)
    })
}

#[derive(Debug)]
struct PoolState {
    reserved: u64,
}
/// Coordinates only drivers sharing this instance in one host process.
#[derive(Clone, Debug)]
pub struct DeviceBudget {
    uuid: String,
    capacity: u64,
    state: Arc<Mutex<PoolState>>,
}
impl DeviceBudget {
    pub fn new(
        uuid: String,
        total_bytes: u64,
        external_reserved_bytes: u64,
    ) -> Result<Self, BudgetError> {
        if !uuid.starts_with("GPU-") || total_bytes == 0 {
            return Err(BudgetError::Invalid);
        }
        let capacity = total_bytes
            .checked_sub(external_reserved_bytes)
            .ok_or(BudgetError::Capacity)?;
        if capacity == 0 {
            return Err(BudgetError::Capacity);
        }
        Ok(Self {
            uuid,
            capacity,
            state: Arc::new(Mutex::new(PoolState { reserved: 0 })),
        })
    }
    pub fn reserved_bytes(&self) -> Result<u64, BudgetError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| BudgetError::Poisoned)?
            .reserved)
    }
    fn reserve(&self, uuid: &str, bytes: u64) -> Result<ResidentLease, BudgetError> {
        if self.uuid != uuid {
            return Err(BudgetError::Scope);
        }
        let mut state = self.state.lock().map_err(|_| BudgetError::Poisoned)?;
        let total = state
            .reserved
            .checked_add(bytes)
            .ok_or(BudgetError::Overflow)?;
        if total > self.capacity {
            return Err(BudgetError::Capacity);
        }
        state.reserved = total;
        Ok(ResidentLease {
            pool: self.clone(),
            bytes,
            owner: None,
        })
    }
}

/// Deliberately has no releasing Drop: uncertain process ownership retains capacity.
pub(crate) struct ResidentLease {
    pool: DeviceBudget,
    bytes: u64,
    owner: Option<(TaskId, PrincipalId)>,
}
impl ResidentLease {
    pub(crate) fn matches(&self, policy: &BudgetPolicy, bytes: u64) -> bool {
        Arc::ptr_eq(&self.pool.state, &policy.pool.state)
            && self.bytes >= bytes
            && self.owner.as_ref() == Some(&(policy.task_id.clone(), policy.principal_id.clone()))
    }
    pub(crate) fn release_confirmed(self) {
        if let Ok(mut state) = self.pool.state.lock()
            && let Some(remaining) = state.reserved.checked_sub(self.bytes)
        {
            state.reserved = remaining;
        }
    }
}
#[derive(Clone, Debug)]
pub struct BudgetPolicy {
    pub pool: DeviceBudget,
    pub bounds: ResidentBounds,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
}
impl BudgetPolicy {
    pub(crate) fn check(
        &self,
        profile: &LocalProfile,
        scopes: &[(&TaskId, &PrincipalId)],
    ) -> Result<u64, BudgetError> {
        if scopes.is_empty()
            || scopes.len() > usize::from(profile.max_sequences)
            || scopes
                .iter()
                .any(|(t, p)| *t != &self.task_id || *p != &self.principal_id)
        {
            return Err(BudgetError::Scope);
        }
        self.validate(profile)
    }
    /// Validate immutable bounds before persistence; reservation still occurs atomically at dispatch.
    pub fn validate(&self, profile: &LocalProfile) -> Result<u64, BudgetError> {
        if self.pool.uuid != self.bounds.scope.device_uuid {
            return Err(BudgetError::Scope);
        }
        if let LocalDevice::Cuda { memory_bytes, .. } = &profile.device
            && self.pool.capacity > memory_bytes.get()
        {
            return Err(BudgetError::Capacity);
        }
        let bytes = resident_bytes(profile, &self.bounds)?;
        if bytes > self.pool.capacity {
            return Err(BudgetError::Capacity);
        }
        Ok(bytes)
    }
    pub(crate) fn reserve(&self, bytes: u64) -> Result<ResidentLease, BudgetError> {
        let mut lease = self.pool.reserve(&self.bounds.scope.device_uuid, bytes)?;
        lease.owner = Some((self.task_id.clone(), self.principal_id.clone()));
        Ok(lease)
    }
}

/// Available shared-pool capacity after disjoint, conservative component bounds.
pub fn usable_tokens_from_memory(
    total: u64,
    external: u64,
    components: &[Option<u64>; 4],
    headroom: u64,
    scalar_bytes: u64,
) -> Result<u64, BudgetError> {
    if total == 0 || !matches!(scalar_bytes, 2 | 4) {
        return Err(BudgetError::Invalid);
    }
    let mut withheld = external
        .checked_add(headroom)
        .ok_or(BudgetError::Overflow)?;
    for component in components {
        withheld = withheld
            .checked_add(component.ok_or(BudgetError::Unqualified)?)
            .ok_or(BudgetError::Overflow)?;
    }
    let available = total.checked_sub(withheld).ok_or(BudgetError::Capacity)?;
    let page_bytes = 32 * 11 * 2 * 4 * 128 * scalar_bytes;
    let usable_pages = (available / page_bytes)
        .checked_sub(1)
        .ok_or(BudgetError::Capacity)?;
    usable_pages
        .checked_mul(32)
        .filter(|n| *n > 0)
        .ok_or(BudgetError::Capacity)
}

#[derive(Clone, Copy, Debug)]
pub struct AllocationRequest {
    pub planner_min: u64,
    pub planner_desired: u64,
    pub planner_output: u64,
    pub executor_count: u16,
    pub executor_context: u64,
    pub executor_output: u64,
    pub usable_tokens: u64,
    pub runtime_ceiling: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub planner_context: u64,
    pub executor_context: u64,
    pub executor_count: u16,
    pub paged_kv_bytes: u64,
    pub runtime_supported: bool,
    pub contexts_within_runtime_ceiling: bool,
}
/// Fixed executor minima, planner-first residual; no silent context/count reduction.
pub fn allocate(request: AllocationRequest) -> Result<Allocation, BudgetError> {
    let r = request;
    if r.executor_count == 0
        || r.executor_count > 15
        || r.planner_min == 0
        || r.planner_min > r.planner_desired
        || r.planner_output == 0
        || r.planner_output >= r.planner_min
        || r.executor_output == 0
        || r.executor_output >= r.executor_context
        || r.runtime_ceiling == 0
        || r.planner_desired > TRAINED_CONTEXT_TOKENS
        || r.executor_context > TRAINED_CONTEXT_TOKENS
    {
        return Err(BudgetError::Invalid);
    }
    let executor_pages = r
        .executor_context
        .div_ceil(32)
        .checked_mul(32)
        .and_then(|n| n.checked_mul(u64::from(r.executor_count)))
        .ok_or(BudgetError::Overflow)?;
    let remaining = r
        .usable_tokens
        .checked_sub(executor_pages)
        .ok_or(BudgetError::Capacity)?;
    let planner_context = r.planner_desired.min(remaining / 32 * 32);
    if planner_context < r.planner_min {
        return Err(BudgetError::Capacity);
    }
    let mut demands = vec![r.executor_context; usize::from(r.executor_count)];
    demands.push(planner_context);
    Ok(Allocation {
        planner_context,
        executor_context: r.executor_context,
        executor_count: r.executor_count,
        paged_kv_bytes: paged_pool_bytes(&demands)?,
        // Heterogeneous five-agent scheduling is not implemented by the current adapter.
        runtime_supported: false,
        contexts_within_runtime_ceiling: planner_context <= r.runtime_ceiling
            && r.executor_context <= r.runtime_ceiling,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_capacity_withholds_all_components_and_the_reserved_page() {
        let page = 32 * 22_528;
        let overhead = 10 + 20 + 30 + 40 + 50 + 60;
        assert_eq!(
            usable_tokens_from_memory(
                2 * page + overhead,
                10,
                &[Some(20), Some(30), Some(40), Some(50)],
                60,
                2
            ),
            Ok(32)
        );
        assert_eq!(
            usable_tokens_from_memory(
                2 * page + overhead - 1,
                10,
                &[Some(20), Some(30), Some(40), Some(50)],
                60,
                2
            ),
            Err(BudgetError::Capacity)
        );
        assert_eq!(
            usable_tokens_from_memory(
                u64::MAX,
                u64::MAX,
                &[Some(1), Some(0), Some(0), Some(0)],
                0,
                4
            ),
            Err(BudgetError::Overflow)
        );
        assert_eq!(
            usable_tokens_from_memory(4 << 30, 0, &[Some(0), None, Some(0), Some(0)], 0, 4),
            Err(BudgetError::Unqualified)
        );
    }
    #[test]
    fn pages_round_independent_demands_and_share_only_reserved_block() {
        for (tokens, allocated) in [(1, 64), (32, 64), (33, 96), (513, 576)] {
            assert_eq!(paged_pool_bytes(&[tokens]), Ok(allocated * 22_528));
        }
        assert_eq!(paged_pool_bytes(&[33, 33]), Ok(160 * 22_528));
        assert_eq!(paged_pool_bytes(&[u64::MAX]), Err(BudgetError::Overflow));
    }
    #[test]
    fn planner_residual_never_shrinks_executors_and_larger_plan_is_not_executable() {
        let mut r = AllocationRequest {
            planner_min: 8192,
            planner_desired: 32768,
            planner_output: 512,
            executor_count: 4,
            executor_context: 8192,
            executor_output: 512,
            usable_tokens: 65536,
            runtime_ceiling: 8192,
        };
        let a = allocate(r).unwrap();
        assert_eq!(
            (a.planner_context, a.executor_context, a.executor_count),
            (32768, 8192, 4)
        );
        assert!(!a.runtime_supported);
        r.usable_tokens = 40960;
        assert_eq!(allocate(r).unwrap().planner_context, 8192);
        r.usable_tokens -= 1;
        assert_eq!(allocate(r), Err(BudgetError::Capacity));
    }
    #[test]
    fn atomic_capacity_retains_uncertain_leases_and_releases_only_confirmed() {
        let pool = DeviceBudget::new("GPU-test".into(), 100, 10).unwrap();
        let lease = pool.reserve("GPU-test", 90).unwrap();
        assert!(matches!(
            pool.reserve("GPU-test", 1),
            Err(BudgetError::Capacity)
        ));
        lease.release_confirmed();
        assert_eq!(pool.reserved_bytes(), Ok(0));
        let lease = pool.reserve("GPU-test", 90).unwrap();
        drop(lease);
        assert_eq!(pool.reserved_bytes(), Ok(90));
        assert!(matches!(
            pool.reserve("GPU-other", 0),
            Err(BudgetError::Scope)
        ));
    }
    #[test]
    fn concurrent_claims_cannot_overbook_or_double_release() {
        let pool = DeviceBudget::new("GPU-test".into(), 100, 0).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let claims = (0..8)
            .map(|_| {
                let pool = pool.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    pool.reserve("GPU-test", 100)
                })
            })
            .collect::<Vec<_>>();
        let leases = claims
            .into_iter()
            .filter_map(|t| t.join().unwrap().ok())
            .collect::<Vec<_>>();
        assert_eq!(leases.len(), 1);
        for lease in leases {
            lease.release_confirmed();
        }
        assert_eq!(pool.reserved_bytes(), Ok(0));
    }
}
