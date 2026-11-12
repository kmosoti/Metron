//! Qualification-only, one-shot authority for exact frozen corpus cases.
use metron_protocol::{
    BoundedText, Digest, LocalOutputSchema, LocalProfile, LocalRole, MAX_CONTEXT, MAX_ENVELOPE,
    MAX_TEXT, PrincipalId, TaskId, canonical_bytes, content_digest,
    inference::{
        CorpusSplit, FrozenCase, SubmissionCohort, parse_frozen_corpus, parse_prompt_recipe,
    },
    structural_digest,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

const RECIPE: &str = "7b8725c7b6328da6a97f487991ca56ac09b05e484a8d8738384c22645a0931a4";
const CALIBRATION: &str = "eeb8d45078ae78ef231dfcbd68b413cb784eeab250c95df206584c3c96aa5842";
const HELDOUT: &str = "6a39feb6e0919ce2dbca7c64005f1cadd38f84a3ac5eab4279ae9032e02a3e5c";

/// Immutable request material; possession alone grants no authority to submit.
pub struct QualificationRequest {
    pub dispatch_id: Digest,
    pub request_digest: Digest,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub action_digest: Digest,
    pub input_digest: Digest,
    pub role: LocalRole,
    pub output_schema: LocalOutputSchema,
    pub context: BoundedText<MAX_CONTEXT>,
    pub profile_digest: Digest,
    pub cohort: SubmissionCohort,
}
/// One consumed submission authority. It cannot be constructed, cloned or deserialized.
///
/// ```compile_fail
/// use metron_harness::qualification::QualificationPermit;
/// let _: QualificationPermit = metron_protocol::parse_envelope(b"{}").unwrap();
/// ```
///
/// ```compile_fail
/// use metron_harness::qualification::QualificationPermit;
/// fn duplicate(permit: &QualificationPermit) -> QualificationPermit { permit.clone() }
/// ```
pub struct QualificationPermit {
    request: QualificationRequest,
}
pub struct QualificationResult {
    pub dispatch_id: Digest,
    pub observation: Option<metron_protocol::LocalObservation>,
    pub rejection: Option<BoundedText<MAX_TEXT>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionKind {
    Cold,
    Warm,
}
pub struct SessionSources<'a> {
    pub recipe: &'a [u8],
    pub corpus: &'a [u8],
    pub calibration: &'a [u8],
    pub calibration_proof: Option<&'a ValidatedCalibration>,
}
/// Evidence checked before the heldout session receives its corpus.
pub struct ValidatedCalibration {
    profile_digest: Digest,
    freeze_digest: Digest,
}
impl ValidatedCalibration {
    pub fn from_reports(
        freeze_path: &Path,
        recipe: &[u8],
        calibration: &[u8],
        warm: &metron_protocol::inference::QualificationReport,
        cold: &metron_protocol::inference::QualificationReport,
        worker_digest: &Digest,
    ) -> io::Result<Self> {
        let expected = crate::qualification_proof::freeze_calibration(
            recipe,
            calibration,
            warm,
            cold,
            worker_digest,
        )
        .map_err(invalid)?;
        let mut bytes = Vec::new();
        File::open(freeze_path)?
            .take((MAX_ENVELOPE + 1) as u64)
            .read_to_end(&mut bytes)?;
        let canonical = canonical_bytes(&expected, MAX_ENVELOPE)
            .map_err(|_| invalid("invalid calibration freeze"))?;
        if bytes != canonical {
            return Err(invalid("durable calibration freeze mismatch"));
        }
        Ok(Self {
            profile_digest: expected.profile_digest.clone(),
            freeze_digest: expected
                .digest()
                .map_err(|_| invalid("invalid freeze digest"))?,
        })
    }
}
impl QualificationPermit {
    pub fn request(&self) -> &QualificationRequest {
        &self.request
    }
}

/// A session is exclusive, bounded, and cannot be reopened after interruption.
pub struct QualificationSession {
    journal: File,
    session_id: Digest,
    profile_digest: Digest,
    profile: LocalProfile,
    kind: SessionKind,
    warmup_succeeded: bool,
    cold_repetition: Option<u8>,
    cases: BTreeMap<String, FrozenCase>,
    warmup_case: FrozenCase,
    issued: BTreeSet<(String, u8, bool)>,
    outstanding: Vec<(Digest, Digest, Digest, LocalOutputSchema, SubmissionCohort)>,
    outstanding_warmup: bool,
    blocked: bool,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn digest(domain: &str, fields: &[&[u8]]) -> io::Result<Digest> {
    structural_digest(domain, fields).map_err(|_| invalid("invalid qualification digest"))
}
fn append(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len())
        .map_err(|_| invalid("qualification journal record too large"))?;
    file.write_all(&length.to_be_bytes())?;
    file.write_all(bytes)?;
    file.sync_all()
}
impl QualificationSession {
    /// The marker must not already exist. A failed or interrupted session is never reopened.
    pub fn create(
        directory: &Path,
        session_id: Digest,
        kind: SessionKind,
        profile: LocalProfile,
        sources: SessionSources<'_>,
    ) -> io::Result<Self> {
        let SessionSources {
            recipe,
            corpus,
            calibration,
            calibration_proof,
        } = sources;
        profile.validate().map_err(|_| invalid("invalid profile"))?;
        if !matches!(profile.max_sequences, 1 | 2 | 4) {
            return Err(invalid("unsupported qualification concurrency"));
        }
        let recipe_digest = content_digest(recipe);
        if recipe_digest.as_str() != RECIPE || profile.prompt_recipe_digest != recipe_digest {
            return Err(invalid("untrusted prompt recipe"));
        }
        parse_prompt_recipe(recipe).map_err(|_| invalid("invalid prompt recipe"))?;
        let parsed = parse_frozen_corpus(corpus).map_err(|_| invalid("invalid frozen corpus"))?;
        if content_digest(calibration).as_str() != CALIBRATION {
            return Err(invalid("untrusted calibration corpus"));
        }
        let calibration =
            parse_frozen_corpus(calibration).map_err(|_| invalid("invalid calibration corpus"))?;
        if calibration.split != CorpusSplit::Calibration
            || calibration.prompt_recipe_sha256 != recipe_digest
        {
            return Err(invalid("invalid calibration binding"));
        }
        let warmup_case = calibration.cases.into_iter().find(|case| matches!(case, FrozenCase::Solver { id, .. } if id.as_str() == "calibration-sum-02"))
            .ok_or_else(|| invalid("missing warmup case"))?;
        let corpus_digest = content_digest(corpus);
        let expected = match parsed.split {
            CorpusSplit::Calibration => CALIBRATION,
            CorpusSplit::Heldout => HELDOUT,
        };
        if corpus_digest.as_str() != expected || parsed.prompt_recipe_sha256 != recipe_digest {
            return Err(invalid("untrusted qualification corpus"));
        }
        let profile_digest = profile
            .digest()
            .map_err(|_| invalid("invalid profile digest"))?;
        if parsed.split == CorpusSplit::Heldout
            && !calibration_proof.is_some_and(|proof| proof.profile_digest == profile_digest)
        {
            return Err(invalid(
                "heldout qualification requires a validated calibration freeze",
            ));
        }
        if parsed.split == CorpusSplit::Heldout && kind == SessionKind::Cold {
            return Err(invalid("cold trials use calibration representatives"));
        }
        let mut cases = BTreeMap::new();
        for case in parsed.cases {
            let id = match &case {
                FrozenCase::Solver { id, .. } | FrozenCase::ToolSelector { id, .. } => {
                    id.as_str().to_owned()
                }
            };
            if cases.insert(id, case).is_some() {
                return Err(invalid("duplicate case"));
            }
        }
        let path = directory.join(format!("session-{}.journal", session_id.as_str()));
        let mut journal = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let header = canonical_bytes(
            &(
                session_id.clone(),
                profile_digest.clone(),
                recipe_digest,
                corpus_digest,
                matches!(kind, SessionKind::Cold),
                calibration_proof.map(|proof| &proof.freeze_digest),
            ),
            MAX_ENVELOPE,
        )
        .map_err(|_| invalid("invalid session header"))?;
        append(&mut journal, &header)?;
        File::open(directory)?.sync_all()?;
        Ok(Self {
            journal,
            session_id,
            profile_digest,
            profile,
            kind,
            warmup_succeeded: false,
            cold_repetition: None,
            cases,
            warmup_case,
            issued: BTreeSet::new(),
            outstanding: Vec::new(),
            outstanding_warmup: false,
            blocked: false,
        })
    }
    /// Append and sync a cohort before returning any authority. `warmup` permits only the frozen representative.
    pub fn admit(&mut self, trials: &[(&str, u8, bool)]) -> io::Result<Vec<QualificationPermit>> {
        if self.blocked
            || !self.outstanding.is_empty()
            || trials.is_empty()
            || trials.len() > usize::from(self.profile.max_sequences)
            || self.issued.len().saturating_add(trials.len()) > self.cases.len() * 3 + 1
        {
            return Err(invalid("qualification session unavailable"));
        }
        if (trials.iter().any(|(_, _, warmup)| *warmup) && trials.len() != 1)
            || (self.kind == SessionKind::Warm
                && !self.warmup_succeeded
                && (trials.len() != 1 || !trials[0].2))
            || (self.kind == SessionKind::Cold
                && trials.iter().any(|(id, _, warmup)| {
                    *warmup || (*id != "calibration-sum-02" && *id != "calibration-tool-00")
                }))
        {
            return Err(invalid("invalid qualification session phase"));
        }
        if self.kind == SessionKind::Cold
            && (self.issued.len().saturating_add(trials.len()) > 2
                || trials
                    .iter()
                    .any(|(_, repetition, _)| *repetition != trials[0].1)
                || self
                    .cold_repetition
                    .is_some_and(|repetition| repetition != trials[0].1))
        {
            return Err(invalid("cold session must contain one fresh-worker trial"));
        }
        let mut proposed = BTreeSet::new();
        let mut permits = Vec::new();
        for &(case_id, repetition, warmup) in trials {
            let key = (case_id.to_owned(), repetition, warmup);
            if repetition >= 3
                || self.issued.contains(&key)
                || !proposed.insert(key)
                || (warmup
                    && (case_id != "calibration-sum-02"
                        || repetition != 0
                        || self.issued.iter().any(|(_, _, w)| *w)))
            {
                return Err(invalid("invalid or repeated qualification trial"));
            }
            let case = if warmup {
                &self.warmup_case
            } else {
                self.cases
                    .get(case_id)
                    .ok_or_else(|| invalid("case outside frozen corpus"))?
            };
            let (context, role, output_schema) = match case {
                FrozenCase::Solver { input, .. } => (
                    canonical_bytes(input, MAX_CONTEXT),
                    LocalRole::Solver,
                    LocalOutputSchema::Sum,
                ),
                FrozenCase::ToolSelector { input, .. } => (
                    canonical_bytes(input, MAX_CONTEXT),
                    LocalRole::ToolSelector,
                    LocalOutputSchema::ToolSelection,
                ),
            };
            let context = context.map_err(|_| invalid("invalid case input"))?;
            let input_digest = content_digest(&context);
            let trial = [repetition, u8::from(warmup)];
            let dispatch_id = digest(
                "qualification-dispatch/v1",
                &[&self.session_id.bytes(), case_id.as_bytes(), &trial],
            )?;
            let role_bytes = canonical_bytes(&(role, output_schema), MAX_ENVELOPE)
                .map_err(|_| invalid("invalid schema"))?;
            canonical_bytes(
                &(
                    &self.profile_digest,
                    &input_digest,
                    role,
                    output_schema,
                    std::str::from_utf8(&context).map_err(|_| invalid("invalid context"))?,
                ),
                MAX_CONTEXT,
            )
            .map_err(|_| invalid("qualification request exceeds context bound"))?;
            let request_digest = digest(
                "qualification-request/v1",
                &[
                    &self.profile_digest.bytes(),
                    &input_digest.bytes(),
                    &role_bytes,
                ],
            )?;
            let action_digest = digest(
                "qualification-action/v1",
                &[
                    &self.session_id.bytes(),
                    &dispatch_id.bytes(),
                    &request_digest.bytes(),
                ],
            )?;
            permits.push(QualificationPermit {
                request: QualificationRequest {
                    dispatch_id,
                    request_digest,
                    task_id: TaskId::new(format!("qual-{}", self.session_id.as_str()))
                        .map_err(|_| invalid("invalid task scope"))?,
                    principal_id: PrincipalId::new("qualification")
                        .map_err(|_| invalid("invalid principal scope"))?,
                    action_digest,
                    input_digest,
                    role,
                    output_schema,
                    context: BoundedText::new(
                        String::from_utf8(context).map_err(|_| invalid("invalid context"))?,
                    )
                    .map_err(|_| invalid("oversized context"))?,
                    profile_digest: self.profile_digest.clone(),
                    cohort: SubmissionCohort {
                        id: BoundedText::new("pending").map_err(|_| invalid("invalid cohort"))?,
                        size: 1,
                        position: 0,
                    },
                },
            });
        }
        let dispatches: Vec<_> = permits
            .iter()
            .map(|p| p.request.dispatch_id.clone())
            .collect();
        let cohort_id = dispatches[0].clone();
        for (position, permit) in permits.iter_mut().enumerate() {
            permit.request.cohort = SubmissionCohort {
                id: BoundedText::new(cohort_id.as_str()).map_err(|_| invalid("invalid cohort"))?,
                size: u16::try_from(trials.len()).map_err(|_| invalid("invalid cohort size"))?,
                position: u16::try_from(position)
                    .map_err(|_| invalid("invalid cohort position"))?,
            };
        }
        let started: Vec<_> = trials
            .iter()
            .zip(&permits)
            .enumerate()
            .map(|(position, (trial, permit))| {
                let r = &permit.request;
                (
                    (trial.0, trial.1, trial.2),
                    (
                        &r.dispatch_id,
                        &r.request_digest,
                        &r.task_id,
                        &r.principal_id,
                        &r.action_digest,
                        &r.input_digest,
                    ),
                    (r.role, r.output_schema, &r.context, &r.profile_digest),
                    (&cohort_id, trials.len(), position),
                )
            })
            .collect();
        let record = canonical_bytes(
            &("started", started),
            metron_protocol::inference::MAX_QUALIFICATION_REPORT,
        )
        .map_err(|_| invalid("invalid started record"))?;
        if let Err(error) = append(&mut self.journal, &record) {
            self.blocked = true;
            return Err(error);
        }
        self.issued.extend(proposed);
        if self.kind == SessionKind::Cold {
            self.cold_repetition = Some(trials[0].1);
        }
        self.outstanding_warmup = trials.iter().any(|(_, _, warmup)| *warmup);
        self.outstanding = permits
            .iter()
            .map(|p| {
                (
                    p.request.dispatch_id.clone(),
                    p.request.request_digest.clone(),
                    p.request.input_digest.clone(),
                    p.request.output_schema,
                    p.request.cohort.clone(),
                )
            })
            .collect();
        Ok(permits)
    }
    /// Persist full results before further dispatch. An unknown result permanently blocks the session.
    pub fn settle(&mut self, results: &[QualificationResult]) -> io::Result<()> {
        if self.blocked || self.outstanding.is_empty() || self.outstanding.len() != results.len() {
            self.blocked = true;
            return Err(invalid("qualification settlement mismatch"));
        }
        for ((dispatch, request, input, schema, cohort), result) in
            self.outstanding.iter().zip(results)
        {
            if dispatch != &result.dispatch_id
                || (result.observation.is_some() && result.rejection.is_some())
            {
                self.blocked = true;
                return Err(invalid("qualification result binding mismatch"));
            }
            if let Some(observation) = &result.observation
                && (observation.input_digest != *input
                    || observation.submission_cohort != *cohort
                    || observation
                        .validate(&self.profile, request, *schema)
                        .is_err())
            {
                self.blocked = true;
                return Err(invalid("qualification observation binding mismatch"));
            }
        }
        let all_known = results
            .iter()
            .all(|r| r.observation.is_some() || r.rejection.is_some());
        let evidence: Vec<_> = results
            .iter()
            .map(|r| (&r.dispatch_id, &r.observation, &r.rejection))
            .collect();
        let record = canonical_bytes(
            &("outcomes", evidence),
            metron_protocol::inference::MAX_QUALIFICATION_REPORT,
        )
        .map_err(|_| invalid("invalid settlement record"))?;
        if let Err(error) = append(&mut self.journal, &record) {
            self.blocked = true;
            return Err(error);
        }
        self.outstanding.clear();
        if self.outstanding_warmup {
            let expected = match &self.warmup_case {
                FrozenCase::Solver { expected, .. } => {
                    Some(metron_protocol::LocalResponse::Sum(expected.clone()))
                }
                _ => None,
            };
            self.warmup_succeeded = all_known
                && results.iter().all(|result| {
                    result
                        .observation
                        .as_ref()
                        .and_then(|observation| observation.response.as_ref())
                        == expected.as_ref()
                });
            if !self.warmup_succeeded {
                self.blocked = true;
            }
            self.outstanding_warmup = false;
        }
        if !all_known {
            self.blocked = true;
        }
        Ok(())
    }
}
