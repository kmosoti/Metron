# ADR 0014: Every prior the design relies on is a hypothesis with a test

Status: accepted. Date: 2026-10-02.

## Context

The repository was built by an agent whose advantage is wide, loosely
structured association: it brings algorithms, results and explanations
from many fields at once, and it brings them as priors, some recalled
from training, some taken from the prior-art review, some derived on the
spot. The laboratory already corrected four such priors during the
build (a selector rule that was backwards, a verification scheme that
tested the wrong rows, a hash that leaked wall-clock time, a handful of
hand-computed expectations). Priors that are never written down cannot be
corrected, and priors that are written down as facts are not tested.

## Decision

- Every prior the design relies on is recorded in
  `docs/research/priors.md`, the priors ledger, with its source (weights,
  the review, a derivation, a primary source), the test that checks it,
  the observation, and its status. A prior with no test is listed as
  untested, not omitted.
- Where a prior can be derived or measured in the laboratory, the
  derivation or measurement is code under test, and the ledger cites the
  test by name. Where it rests on a primary source, the source is
  fetched and quoted.
- When a measurement contradicts a prior, the code follows the
  measurement, the prior is marked falsified with the numbers, and any
  explanation written under the old prior is refined in the ledger and,
  if a decision depended on it, superseded by a new ADR.
- Each entry also records the perspectives from which the same result
  was examined (information-theoretic, Bayesian, algorithm-selection,
  cost accounting, identifiability, implementation), because a result
  that holds from one angle and fails from another is where the
  interesting findings have been.

## First application

The ledger's first pass validated the NPN class counts by Burnside's
lemma and against OEIS A000370, the bounded sampler by chi-square, the
greedy query rule against the exact optimal tree, the bootstrap by a
coverage simulation, and the family posterior by a calibration curve.
It falsified, in part, the prior that a Bloom filter's false-positive
rate follows the textbook formula with double hashing at the sizes used
here: double hashing measured 0.032, independent hashing on the store's
structured keys 0.028, an idealised simulation 0.022 against the
textbook 0.022, and twelve different hash salts on random tables ranged
from 0.015 to 0.029. The formula is an expectation over hash functions
and uniform keys; one deployed hash on a 128-key universe is not that
ensemble. The implementation now uses independent hashes and the test
tolerance records the deployment-specific deviation. The pass also
refined the explanation in ADR 0011: literal cross-family overlap is
about ten percent, the family posterior is well calibrated, and the
selector fails because short probe prefixes do not separate the
families, not because the rule misreads them.

## Consequences

- `AGENTS.md` requires a ledger entry for any new prior a change relies
  on, and a test or a source for it before the change is merged.
- The ledger is append-only in spirit: entries change status, they do
  not disappear.
