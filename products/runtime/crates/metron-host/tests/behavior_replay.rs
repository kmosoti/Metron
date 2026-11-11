//! A valid hash chain cannot substitute for rerunning cognition.
use metron_agent::SumAgent;
use metron_harness::{
    orchestrator::{self, NoObserver},
    replay,
};
use metron_protocol::{
    domain::Transition,
    store::{
        AuditSnapshot, CommandData, EngineMode, EvidenceSnapshot, Genesis, GrantSnapshot,
        InboxInput, InboxSnapshot, IntentSnapshot, JournalEvent, JournalPage, JournalRecord,
        JournalRecordData, Projection, RuntimeSnapshot, SnapshotPage, StoreCommand, StoreError,
        StoreOperation, StorePort, UnpostedDebit,
    },
    types::{
        BoundedText, CommandId, Digest, MAX_INTERNAL, PrincipalId, SchemaVersion, SqlU64, TaskId,
        canonical_bytes, content_digest, structural_digest,
    },
};
use metron_store_sqlite::SqliteStore;

fn alter(transition: &mut Transition) {
    if let Transition::Decision { intent, .. } = transition
        && intent.ordinal == 0
    {
        intent.ordinal = 7;
    }
}
struct Tampered<S> {
    inner: S,
    records: Vec<JournalRecord>,
    head: Digest,
}
impl<S: RuntimeSnapshot> Tampered<S> {
    fn new(mut inner: S) -> Self {
        let mut records = Vec::new();
        let mut cursor = None;
        loop {
            let page = inner.journal_page(cursor).unwrap();
            records.extend(page.records);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        let mut previous = records.first().unwrap().data().previous.clone();
        for record in &mut records {
            let original = record.data();
            let JournalEvent::CommandCommitted { command, result } = &original.event;
            let mut data: CommandData = command.clone().into();
            if let StoreOperation::CommitTransition { transition, .. } = &mut data.operation {
                alter(transition);
            }
            let event = JournalEvent::CommandCommitted {
                command: StoreCommand::new(data).unwrap(),
                result: result.clone(),
            };
            let digest = structural_digest(
                "journal-entry/v1",
                &[
                    &previous.bytes(),
                    &original.seq.get().to_be_bytes(),
                    &canonical_bytes(&event, MAX_INTERNAL).unwrap(),
                ],
            )
            .unwrap();
            *record = JournalRecord::new(JournalRecordData {
                seq: original.seq,
                previous: previous.clone(),
                event,
                digest: digest.clone(),
            })
            .unwrap();
            previous = digest;
        }
        Self {
            inner,
            records,
            head: previous,
        }
    }
}
impl<S: RuntimeSnapshot> AuditSnapshot for Tampered<S> {
    fn projection(&mut self, task: &TaskId) -> Result<Projection, StoreError> {
        let mut p = self.inner.projection(task)?;
        p.replay_head = self.head.clone();
        Ok(p)
    }
    fn journal_page(&mut self, after: Option<SqlU64>) -> Result<JournalPage, StoreError> {
        let start = after.map_or(0, |v| v.get() as usize);
        let records: Vec<_> = self.records.iter().skip(start).take(16).cloned().collect();
        let end = start + records.len();
        Ok(JournalPage {
            records,
            next: if end < self.records.len() {
                Some(SqlU64::new(end as u64).unwrap())
            } else {
                None
            },
        })
    }
    fn pending_inbox(&mut self, task: &TaskId) -> Result<Option<InboxInput>, StoreError> {
        self.inner.pending_inbox(task)
    }
}
impl<S: RuntimeSnapshot> RuntimeSnapshot for Tampered<S> {
    fn intent(&mut self, id: &Digest) -> Result<Option<IntentSnapshot>, StoreError> {
        self.inner.intent(id)
    }
    fn intents(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<IntentSnapshot>, StoreError> {
        self.inner.intents(task, after)
    }
    fn grant(&mut self, principal: &PrincipalId) -> Result<GrantSnapshot, StoreError> {
        self.inner.grant(principal)
    }
    fn evidence(
        &mut self,
        id: &Digest,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<EvidenceSnapshot>, StoreError> {
        self.inner.evidence(id, after)
    }
    fn inbox(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<InboxSnapshot>, StoreError> {
        let mut page = self.inner.inbox(task, after)?;
        for row in &mut page.records {
            if let Some(t) = &mut row.transition {
                alter(t);
            }
        }
        Ok(page)
    }
    fn unposted_debits(
        &mut self,
        task: &TaskId,
        after: Option<SqlU64>,
    ) -> Result<SnapshotPage<UnpostedDebit>, StoreError> {
        self.inner.unposted_debits(task, after)
    }
}
#[test]
fn valid_rehashed_chain_cannot_hide_changed_transition() {
    let root = std::env::temp_dir().join(format!("metron-replay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).unwrap();
    let task_id = TaskId::new("replay").unwrap();
    let task =
        metron_testkit::sum_task(task_id.clone(), PrincipalId::new("principal").unwrap()).unwrap();
    let genesis = Genesis {
        mode: EngineMode::Agent,
        engine_version: BoundedText::new("sum-agent/v1").unwrap(),
        require_publication_approval: false,
        policy_digest: content_digest(b"metron-alpha0-host-policy-v1"),
        local_profile: None,
    };
    let create = StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new("create").unwrap(),
        operation: StoreOperation::CreateTask { task, genesis },
    })
    .unwrap();
    let mut store = SqliteStore::create(root.join("runtime.sqlite")).unwrap();
    store.apply(create).unwrap();
    assert_eq!(
        orchestrator::drive(&mut store, &task_id, &SumAgent, &root, &mut NoObserver).unwrap(),
        orchestrator::DriveStatus::Complete
    );
    let snapshot = store.audit().unwrap();
    let mut tampered = Tampered::new(snapshot);
    let mut previous = tampered.records[0].data().previous.clone();
    for record in &tampered.records {
        let data = record.data();
        assert_eq!(data.previous, previous);
        let digest = structural_digest(
            "journal-entry/v1",
            &[
                &previous.bytes(),
                &data.seq.get().to_be_bytes(),
                &canonical_bytes(&data.event, MAX_INTERNAL).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(data.digest, digest);
        previous = digest;
    }
    assert_eq!(previous, tampered.head);
    assert!(!replay::behavior(&mut tampered, &task_id, &SumAgent).unwrap());
    drop(tampered);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
