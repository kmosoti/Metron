# Priors ledger

Every prior the design relies on, with its source, the test or primary
source that checks it, what was observed, and its status (ADR 0014).
Sources: **weights** (recalled from training), **review** (the prior-art
review in this directory), **derivation** (worked out in the session),
**source** (a primary source fetched and quoted). Status: **validated**,
**refined**, **falsified**, **untested**. Entries change status; they are
not removed.

## Ledger

| # | Prior | Source | Test or source | Observation | Status |
|---|---|---|---|---|---|
| 1 | NPN classes of functions on n inputs: 1, 2, 4, 14, 222, 616126, 200253952527184 | weights, review | `npn::class_count_burnside` (Burnside over the hyperoctahedral group with output negation; tests `burnside_*`); OEIS A000370 fetched: terms 1, 2, 4, 14, 222, 616126, 200253952527184, 263735716028826576482466871188128, … | Derivation equals the recalled values and the source through n = 6 | validated |
| 2 | The NPN canonicaliser's pruned candidate set is an orbit invariant, so its minimum is an exact class representative | derivation | Exhaustive counts at n ≤ 4 equal the Burnside counts; `canonical_form_is_invariant_under_random_transforms` at n = 5..8, all six families | Agrees; a canonicaliser that merged too much or too little would miss 222 | validated (exact at n ≤ 4, by property at n = 5..8) |
| 3 | Any membership-query identification of a class of N functions needs ⌈log2 N⌉ queries in the worst case | derivation | `bounds::information_lower_bound`; `optimal_query_depth ≥ bound` in tests | Holds | validated |
| 4 | Greedy "split the survivors" is near-optimal (Dasgupta 2004; Golovin & Krause 2010) | review | `greedy_is_near_optimal_on_random_small_classes`: 60 random explicit classes of 8–16 members at arity 4–5, greedy worst-case depth over exact optimal depth | mean ratio 1.004, max 1.25, equal in 59 of 60 | validated empirically at this scale; the theorems themselves not read |
| 5 | Hegedűs's XTD bracket; Hellerstein et al.; ASlib "gap closed"; Milli, Lieder & Griffiths 2017; Berlot-Attwell et al.; Kleyko et al.; Clarkson et al. | review | none | Cited as the review states them | untested |
| 6 | The SBS–VBS gap in ADR 0010 is log2 of the number of families: log2 240 − log2 40 | derivation, measurement | `experiments/reports/headroom-arity{5,6}-*.md` | greedy-pool 8.03 / 7.98 vs log2 240 = 7.91; restricted 5.32–5.50 vs log2 40 = 5.32; gap 2.64 / 2.62 vs log2 6 = 2.585; residual ≈ 0.06 is greedy's imbalance | validated |
| 7 | Under a uniform prior over pool members the family posterior is proportional to surviving members, so "most survivors" is the Bayes rule | derivation | `diagnostics::family_posterior_calibration` (`most_survivors_posterior_is_informative…`), arity 5, seed 101 | Calibrated: at k = 6, buckets with mean mass 0.28 / 0.54 / 0.75 have accuracy 0.21 / 0.58 / 0.75; at k = 2, mass 0.22 / 0.33 give 0.27 / 0.50 | validated; hidden assumption made explicit: targets are drawn uniformly over families, which equals uniform over members only because families are equal-sized (240 = 6 × 40) |
| 8 | "Families overlap heavily", the explanation offered in ADR 0011 | assertion | `diagnostics::pool_crosstab` (`labels_are_not_properties_of_the_functions`) on the headroom pools | Arity 5: 17 of 200 non-monotone-labelled members are monotone, 3 of 200 non-affine-labelled are affine; arity 6: 18 and 3. About ten percent, not heavy | refined: the selector fails because short probe prefixes genuinely do not separate the families (the calibrated posterior of the top family rarely exceeds 0.6 after six probes), not because of literal class overlap. ADR 0011's decision stands |
| 9 | Bloom filter false-positive rate ≈ (1 − e^(−hn/m))^h, minimised near h = (m/n) ln 2, and double hashing behaves like independent hashing | weights | `bloom_false_positive_rate_matches_the_textbook_formula`; idealised simulation; salt sweep | Double hashing on the store: 0.0323. Independent SHA-256 positions on the store: 0.0279. Idealised random keys and random hash: 0.0218 (textbook 0.0216). One fixed hash on the 128-key universe, random tables: 0.015 to 0.029 across twelve salts, mean 0.0227 | falsified in part: the formula is an expectation over hash functions and uniform keys; a single deployed hash on a 128-key universe deviates by ±30%, and the store's structured key sets deviate further. Implementation switched to independent hashes; the test's ±0.01 tolerance documents the deployment-specific deviation |
| 10 | A 95% percentile bootstrap interval for a mean covers about 95% of the time | weights | `percentile_bootstrap_intervals_have_about_nominal_coverage`: 300 replications, n = 40 uniform samples, 400 resamples | coverage 0.957 | validated |
| 11 | The interquartile mean here equals the rliable definition | weights | hand-checked small cases only (`iqm_trims_the_tails`) | Not compared to the reference implementation | untested |
| 12 | Lemire's bounded sampling is unbiased | weights | `below_is_uniform_for_a_non_power_of_two`: 10⁶ draws over 1,000 bins | chi-square 1030.5 on 999 degrees of freedom (0.1% critical ≈ 1144) | validated |
| 13 | The xoshiro256** and splitmix64 constants are correct | weights | statistical tests only; no reference output vectors | Determinism and uniformity hold; conformance to the reference generator unverified | untested |
| 14 | Generalising an episode's trace by repeating its longest repeated unit yields a reusable composition | design | `promotion.rs` test; `experiments/reports/promotion-arity5-*.md` | Greedy and exhaustive compositions promoted at 100% held-out; the extracted greedy fallback beats its parent (100% vs 80%) | validated on this data |
| 15 | A few verification probes refute a non-affine target's affine fit | design | `lab_strategies.rs`; headroom reports | Lowest-row verification: 46% false commits; greedy verification: 7.7% (arity 5), 17% (arity 6) | falsified, then improved; still weak, recorded in ADR 0010 |
| 16 | Journals replay to the same head hash | design | `identical_runs_produce_identical_journals` | First version leaked receipt wall-clock time into the chain | falsified, then fixed (replay projection) |
| 17 | The family with the fewest surviving hypotheses is the right one to commit to | design | selector sweeps, `experiments/reports/selectors-arity{5,6}-*.md` | 6 to 17% solved | falsified; replaced by the Bayes rule (entry 7) |
| 18 | Hyperdimensional codes cannot win on recall per byte for the laboratory's native objects | review | `experiments/reports/retrieval-arity6-*.md` | 1,024 bytes per entry for 96 to 99% recall against 16 bytes at 100% | validated |
| 19 | Burnside's lemma applied to the NPN group counts classes | weights (the lemma), derivation (its application) | agreement with OEIS and with exhaustive enumeration | Agrees | validated |

## Meta-insights

- Priors about established mathematics (entries 1, 3, 10, 12, 19) all
  held. Priors about my own designs (14 to 17) failed in three of four
  cases on first measurement. Risk concentrates in the untested glue
  between known results, not in the known results.
- The laboratory corrected explanations, not only numbers. ADR 0011's
  decision was right for a reason I had stated wrongly; the crosstab and
  the calibration curve replaced "heavy overlap" with "probe-level
  inseparability of a well-calibrated posterior". A calibrated posterior
  that stays low is the honest signature of "the data do not carry this
  information", and it is distinguishable from "the rule is wrong".
- A formula that is an expectation over an ensemble (hash functions,
  uniform keys) does not bind one deployment on a tiny universe. The
  check that mattered was the per-deployment measurement, and the
  idealised simulation was needed to tell "the formula is wrong here"
  from "the implementation is wrong".
- Where the laboratory was most useful was where my confidence was
  highest: the selector rule felt obvious and was backwards.
- Every prior that was tested was tested by code that is now part of the
  suite, so the ledger does not decay: the next change that breaks a
  validated prior fails a test.

## Perspectives on the headroom result

The same measurement (ADR 0010, 0011) read from six angles; where they
disagree is where the finding lives.

- **Information.** The gap is log2 6 bits: exactly the family identity.
  From this angle the gap looks earnable.
- **Bayesian.** The family posterior after k probes is calibrated and
  low; the information is not in the probes. From this angle the gap is
  not earnable by any rule on those probes.
- **Algorithm selection.** The virtual best solver is keyed on a latent
  label. ASlib-style headroom overstates realisable gain whenever the
  per-instance best is keyed on something not computable from the
  instance.
- **Cost accounting.** With a PAR-style penalty, a 5% wrong-family rate
  costs more than the whole gap; without a penalty the selectors still
  lose. The conclusion is robust to the penalty, which was not obvious.
- **Identifiability.** Literal label-versus-property disagreement is
  about ten percent; the inseparability is a property of the pool's
  geometry under few probes, which is why a bigger pool or disjoint
  signatures are the stated reopening condition.
- **Implementation.** The whole finding rests on the same deterministic
  harness that validated itself against Burnside, OEIS and a coverage
  simulation; the measurement code is under the same test discipline as
  the thing it measures.
