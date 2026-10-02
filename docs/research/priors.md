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
| 5 | Hegedűs's XTD bracket; Hellerstein et al.; ASlib "gap closed"; Milli, Lieder & Griffiths 2017; Berlot-Attwell et al.; Kleyko et al.; Clarkson et al. | review | primary sources fetched: Crossref records, arXiv abstracts, the ASlib paper, Lindauer et al. 2018 and Hanneke 2007 (see "Citation checks") | Every bibliographic record matches the review; the bracket, the gap-closed metric and its orientation, the 38% figure and the library-learning negative result read as the review states them; one link in the review's table is attached to the wrong paper | validated (the Hegedűs bracket through Hanneke's restatement, not the COLT paper); one link refined |
| 6 | The SBS–VBS gap in ADR 0010 is log2 of the number of families: log2 240 − log2 40 | derivation, measurement | `experiments/reports/headroom-arity{5,6}-*.md` | greedy-pool 8.03 / 7.98 vs log2 240 = 7.91; restricted 5.32–5.50 vs log2 40 = 5.32; gap 2.64 / 2.62 vs log2 6 = 2.585; residual ≈ 0.06 is greedy's imbalance | validated |
| 7 | Under a uniform prior over pool members the family posterior is proportional to surviving members, so "most survivors" is the Bayes rule | derivation | `diagnostics::family_posterior_calibration` (`most_survivors_posterior_is_informative…`), arity 5, seed 101 | Calibrated: at k = 6, buckets with mean mass 0.28 / 0.54 / 0.75 have accuracy 0.21 / 0.58 / 0.75; at k = 2, mass 0.22 / 0.33 give 0.27 / 0.50 | validated; hidden assumption made explicit: targets are drawn uniformly over families, which equals uniform over members only because families are equal-sized (240 = 6 × 40) |
| 8 | "Families overlap heavily", the explanation offered in ADR 0011 | assertion | `diagnostics::pool_crosstab` (`labels_are_not_properties_of_the_functions`) on the headroom pools | Arity 5: 17 of 200 non-monotone-labelled members are monotone, 3 of 200 non-affine-labelled are affine; arity 6: 18 and 3. About ten percent, not heavy | refined: the selector fails because short probe prefixes genuinely do not separate the families (the calibrated posterior of the top family rarely exceeds 0.6 after six probes), not because of literal class overlap. ADR 0011's decision stands |
| 9 | Bloom filter false-positive rate ≈ (1 − e^(−hn/m))^h, minimised near h = (m/n) ln 2, and double hashing behaves like independent hashing | weights | `bloom_false_positive_rate_matches_the_textbook_formula`; idealised simulation; salt sweep | Double hashing on the store: 0.0323. Independent SHA-256 positions on the store: 0.0279. Idealised random keys and random hash: 0.0218 (textbook 0.0216). One fixed hash on the 128-key universe, random tables: 0.015 to 0.029 across twelve salts, mean 0.0227 | falsified in part: the formula is an expectation over hash functions and uniform keys; a single deployed hash on a 128-key universe deviates by ±30%, and the store's structured key sets deviate further. Implementation switched to independent hashes; the test's ±0.01 tolerance documents the deployment-specific deviation |
| 10 | A 95% percentile bootstrap interval for a mean covers about 95% of the time | weights | `percentile_bootstrap_intervals_have_about_nominal_coverage`: 300 replications, n = 40 uniform samples, 400 resamples | coverage 0.957 | validated |
| 11 | The interquartile mean here equals the rliable definition | weights | `rliable/metrics.py` fetched: `aggregate_iqm` is `scipy.stats.trim_mean(scores, 0.25)`, which drops `floor(n/4)` values from each end; `iqm_matches_the_rliable_reference` against scipy 1.17.1 | The fractional-weight interquartile mean agreed only when n mod 4 = 0 (differences up to 4.1 on 0–30 data at n = 3, 0.2 at n ≈ 40). Every committed report pools n = 300, so no reported number changes; a per-family or split-restricted IQM would have | falsified for n mod 4 ≠ 0, then fixed: `iqm` now is the trimmed mean, and the reference values are a test |
| 12 | Lemire's bounded sampling is unbiased | weights | `below_is_uniform_for_a_non_power_of_two`: 10⁶ draws over 1,000 bins | chi-square 1030.5 on 999 degrees of freedom (0.1% critical ≈ 1144) | validated |
| 13 | The xoshiro256** and splitmix64 constants are correct | weights | `matches_the_reference_implementation`: Blackman and Vigna's `xoshiro256starstar.c` and `splitmix64.c` fetched from prng.di.unimi.it, compiled, seeded the way `seed_from_u64` seeds; first six outputs for seeds 0, 1, 42, 0xDEADBEEF and 2^64 − 1 | All thirty outputs equal | validated |
| 14 | Generalising an episode's trace by repeating its longest repeated unit yields a reusable composition | design | `promotion.rs` test; `experiments/reports/promotion-arity5-*.md` | Greedy and exhaustive compositions promoted at 100% held-out; the extracted greedy fallback beats its parent (100% vs 80%) | validated on this data |
| 15 | A few verification probes refute a non-affine target's affine fit | design | `lab_strategies.rs`; headroom reports | Lowest-row verification: 46% false commits; greedy verification: 7.7% (arity 5), 17% (arity 6) | falsified, then improved; still weak, recorded in ADR 0010 |
| 16 | Journals replay to the same head hash | design | `identical_runs_produce_identical_journals` | First version leaked receipt wall-clock time into the chain | falsified, then fixed (replay projection) |
| 17 | The family with the fewest surviving hypotheses is the right one to commit to | design | selector sweeps, `experiments/reports/selectors-arity{5,6}-*.md` | 6 to 17% solved | falsified; replaced by the Bayes rule (entry 7) |
| 18 | Hyperdimensional codes cannot win on recall per byte for the laboratory's native objects | review | `experiments/reports/retrieval-arity6-*.md` | 1,024 bytes per entry for 96 to 99% recall against 16 bytes at 100% | validated |
| 19 | Burnside's lemma applied to the NPN group counts classes | weights (the lemma), derivation (its application) | agreement with OEIS and with exhaustive enumeration | Agrees | validated |
| 20 | An always-correct adaptive strategy averages at least the entropy of the target distribution in queries (Shannon; Kraft's inequality on the query tree's leaves) | weights | `bounds::expected_queries_are_bounded_below_by_entropy`: exact optimal expected depth by DP on 40 random explicit classes of 6–14 members at arity 4–5, uniform and skewed weights | The optimum is never below the entropy; mean slack 0.066 bits; greedy's mean depth equals the optimum on average (ratio 1.000) | validated |
| 21 | The SBS–VBS gap measures the headroom a router can realise (the ASlib convention used in ADR 0010) | review | entropy floor attached to the committed headroom and selector reports: floor 7.907 at both arities (log2 240) | The single best solver sits 0.126 (arity 5) and 0.070 (arity 6) above the floor, while the gap is 2.64 and 2.62. At most 4.8% and 2.7% of the gap is reachable by any strategy that is not told the family; the virtual best sits 2.5 probes below the floor | falsified as a measure of realisable headroom: the gap measures the value of the label. Realisable headroom is SBS minus the floor. ADR 0011's negative result follows from it without running a selector |
| 22 | Bigger pools would reopen routing (ADR 0011's first reopening candidate) | derivation (ADR 0011) | the entropy floor with entry 4: greedy over the published union stays within a fraction of a probe of the floor at any pool size | Ruled out: a larger published pool raises the floor and the greedy cost together | falsified by derivation; ADR 0015 replaces the candidate with a structure-keyed task set that publishes no pool |
| 23 | Adding a field with a serde default to the manifest schema leaves existing manifests' hashes, and so the reports named by them, unchanged | design | `every_committed_report_names_the_hash_of_its_manifest`: recompute each committed report's manifest hash | Five of six reports no longer matched. `selector` and `extra_cost_models` moved the two M2 reports; `retrieval` moved the M3 control and M4 reports. No single serialisation reproduces all six | falsified, then fixed: empty `retrieval` is skipped again, which restores four; the two M2 reports carry a verified record in `experiments/reports/provenance.json`; new fields serialise only when set, and the test catches any future drift |
| 24 | NPN-clean splits within each task set are clean enough to pool seeds for learning | design | `class_hash_splits_stay_clean_when_seeds_are_pooled`; the arity-8 structure-keyed sets | Structural classes have few NPN classes (eight affine, nine junta types at arity 8), so a class dealt to train under one seed lands in test under another. Pooled hygiene fails | falsified before it could contaminate a result; the structure-keyed task sets assign splits by hashing each class's canonical form, and pooled hygiene is checked |

## Citation checks (entry 5)

What the review cites, what the primary source says, and whether the
repository relies on it.

| Citation | Source checked | Observation | Relied on by |
|---|---|---|---|
| Hegedűs 1995, COLT, pp. 108–117 | Crossref record (doi 10.1145/225298.225311); Hanneke 2007, Theorem 1, which restates the result | max{XTD, log2 \|C\|} ≤ #MQ(C) ≤ XTD · log2 \|C\|, the upper bound by MembHalving; Hegedűs's greedy ordering tightens it to about 2 (XTD / log XTD) log2 \|C\|. The arXiv link the review's table attaches to this row (1702.05677) is a 2017 paper on recursive teaching dimension by Hu, Wu, Li and Wang, not Hegedűs | the lower half of the bracket only (entry 3); the lab never computes XTD |
| Hellerstein, Pillaipakkamnatt, Raghavan & Wilkins 1996 | Crossref record: JACM 43(5):840–862 (doi 10.1145/234752.234755) | Record matches; the review's "membership and equivalence queries" reading not checked against the paper. Hanneke's footnote shows the paper also tightens the membership-only halving bound, so "not usable without equivalence queries" is too strong as a blanket statement | nothing |
| Bischl et al. 2016, ASlib | the AIJ paper (PDF), Figure 5 caption | "how much of the gap between the single best and the virtual best solver in terms of PAR10 score was closed by each model. That is, a value of 0 corresponds to the single best solver and a value of 1 to the virtual best." Same orientation as `gap_closed` in `headroom.rs` | the headroom report's gap-closed column and ADR 0010's success criterion |
| Lindauer, van Rijn & Kotthoff 2018 (arXiv 1805.01214), "the best 2017 system still left a 38% gap" | the paper (PDF), Equation 2 and Table 4 | Equation 2 normalises so that 0 is the VBS and 1 the SBS (the 2017 competition itself used the opposite orientation, footnote 3); Table 4 gives ASAP.v2 an average gap of 0.38, so the winner closed 62% and left 38%. The review's reading is right; the number is meaningless without the orientation | the expectation in ADR 0010 that even good selectors leave a large share of the gap |
| Milli, Lieder & Griffiths 2017 | Crossref record: AAAI 31(1) (doi 10.1609/aaai.v31i1.11156) | Record matches; content not read | ADR 0011's remark that a control can beat a selector |
| Lieder & Griffiths 2017 | Crossref record: Psychological Review 124(6):762–794 (doi 10.1037/rev0000075) | Record matches | nothing yet |
| Berlot-Attwell, Rudzicz & Si 2024 (arXiv 2410.20274) and 2025 (arXiv 2504.03048) | arXiv abstracts | "function reuse is extremely infrequent on miniF2F and MATH … self-correction and self-consistency are the primary drivers"; the follow-up "finds no evidence of the direct reuse of learned lemmas" in LEGO-Prover. As the review states | ADR 0012: promotion is judged on held-out targets, not on reported reuse |
| Kleyko, Rachkovskij, Osipov & Rahimi, Parts I and II | Crossref records and DOI resolution: ACM CSUR 55(6) (doi 10.1145/3538531), 55(9) (doi 10.1145/3558000) | Records match; the DOI resolves to the ACM library | ADR 0013 as background; the decision rests on the measurement |
| Clarkson, Ubaru & Yang (arXiv 2301.10352) | arXiv abstract | "Capacity Analysis of Vector Symbolic Architectures": dimension bounds for set membership and intersection-size queries in MAP-I, MAP-B and two sparse binary VSAs. As the review states | ADR 0013 as background |
| Agarwal et al. 2021 (rliable) | `rliable/metrics.py` fetched | IQM is `scipy.stats.trim_mean(scores, 0.25)`; see entry 11 | every IQM column in the reports |

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
- The cheapest experiment was a calculation. The entropy floor, one line
  of arithmetic per task set, predicts ADR 0011's negative result to
  within a tenth of a probe; the selector sweep that established it cost
  thousands of episodes. Before building a router, compute the floor.
- Two silent failures were in bookkeeping, not science: report names
  drifting away from their manifests' hashes, and split hygiene that held
  per seed but not pooled. Both would have stayed invisible because every
  number in every report was still right. Provenance needs its own tests.
- A virtual best solver can sit below what any honest strategy can
  reach. When the per-task best is keyed by something the strategies are
  never told, the gap prices that something, not the router.
- "Equals the reference" is a prior about a convention, not about
  mathematics, and conventions hide in rounding. The interquartile mean
  was right at every n divisible by four, which is every n the reports
  use, and wrong everywhere else; a hand check on convenient sizes
  cannot see that. The fix was to adopt the reference's rounding and
  pin it with values from the reference implementation.
- Priors recalled as bare numbers (the generator constants, the NPN
  counts) were exact; priors recalled as sentences (the Bloom formula's
  scope, "heavy overlap", "equals rliable") carried the errors. The
  reliable part of recall is the part with no interpretation in it.
- A citation can be right and its number still unusable: 38% is the
  share of the gap left or closed depending on which end is zero, and
  the competition and the paper reporting it chose opposite ends. The
  check that mattered was the normalisation, not the figure.
- The bibliographic layer of the review held completely (every record
  matched), while one of its links pointed at the wrong paper. Links are
  the weakest part of a generated review; records and quotations were
  reliable.

## Perspectives on the headroom result

The same measurement (ADR 0010, 0011) read from six angles; where they
disagree is where the finding lives.

- **Information.** The gap is log2 6 bits: exactly the family identity.
  Read naively, the gap looks earnable. Read correctly, it is not: the
  virtual best sits 2.5 probes below the entropy of the target
  distribution, which no strategy without the label can beat, so the
  information perspective alone predicts the negative result (entries
  20 and 21). The first reading of this bullet was wrong; it is kept
  here as written so the correction stays visible.
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
