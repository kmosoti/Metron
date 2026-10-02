//! Reporting statistics: interquartile mean and stratified bootstrap
//! confidence intervals, after Agarwal et al. (2021).

use metron_core::rng::Rng;

/// Arithmetic mean; `0` for an empty slice.
#[must_use]
pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        0.0
    } else {
        xs.iter().sum::<f64>() / xs.len() as f64
    }
}

/// Interquartile mean: the mean of the middle half of the sorted values,
/// with fractional weights at the quartile boundaries.
#[must_use]
pub fn iqm(xs: &[f64]) -> f64 {
    let n = xs.len();
    if n == 0 {
        return 0.0;
    }
    let mut sorted = xs.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite values"));
    let lo = n as f64 * 0.25;
    let hi = n as f64 * 0.75;
    let mut total = 0.0;
    let mut weight = 0.0;
    for (i, &x) in sorted.iter().enumerate() {
        let start = i as f64;
        let end = start + 1.0;
        let overlap = (end.min(hi) - start.max(lo)).max(0.0);
        if overlap > 0.0 {
            total += x * overlap;
            weight += overlap;
        }
    }
    if weight > 0.0 {
        total / weight
    } else {
        mean(&sorted)
    }
}

/// A percentile bootstrap interval for `statistic` over `groups`,
/// resampling within each group (stratum) so every stratum keeps its size.
///
/// Returns `(point, low, high)` at confidence `1 - alpha`.
#[must_use]
pub fn stratified_bootstrap(
    groups: &[Vec<f64>],
    statistic: fn(&[f64]) -> f64,
    resamples: usize,
    alpha: f64,
    seed: u64,
) -> (f64, f64, f64) {
    let pooled: Vec<f64> = groups.iter().flatten().copied().collect();
    let point = statistic(&pooled);
    if pooled.is_empty() || resamples == 0 {
        return (point, point, point);
    }
    let mut rng = Rng::seed_from_u64(seed);
    let mut stats = Vec::with_capacity(resamples);
    let mut sample = Vec::with_capacity(pooled.len());
    for _ in 0..resamples {
        sample.clear();
        for g in groups {
            for _ in 0..g.len() {
                sample.push(g[rng.below_usize(g.len())]);
            }
        }
        stats.push(statistic(&sample));
    }
    stats.sort_by(|a, b| a.partial_cmp(b).expect("finite values"));
    let idx = |q: f64| -> f64 {
        let pos = (q * (stats.len() - 1) as f64).round() as usize;
        stats[pos.min(stats.len() - 1)]
    };
    (point, idx(alpha / 2.0), idx(1.0 - alpha / 2.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iqm_trims_the_tails() {
        let xs = [1.0, 2.0, 3.0, 4.0, 100.0, 5.0, 6.0, 7.0];
        assert!((iqm(&xs) - 4.5).abs() < 1e-9);
        assert_eq!(iqm(&[]), 0.0);
        assert_eq!(iqm(&[3.0]), 3.0);
        assert!((iqm(&[1.0, 2.0, 3.0]) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn percentile_bootstrap_intervals_have_about_nominal_coverage() {
        // Prior: a 95% percentile bootstrap interval for a mean covers the
        // true mean about 95% of the time (a little less at small n).
        let mut rng = Rng::seed_from_u64(2024);
        let reps = 300;
        let n = 40;
        let mut covered = 0;
        for r in 0..reps {
            let sample: Vec<f64> = (0..n).map(|_| rng.next_f64()).collect();
            let (_, lo, hi) = stratified_bootstrap(&[sample], mean, 400, 0.05, 1_000 + r);
            if lo <= 0.5 && 0.5 <= hi {
                covered += 1;
            }
        }
        let coverage = covered as f64 / reps as f64;
        println!("percentile bootstrap, n = {n}, {reps} replications: coverage {coverage:.3}");
        assert!((0.88..=0.99).contains(&coverage), "coverage {coverage}");
    }

    #[test]
    fn bootstrap_interval_contains_the_point_and_is_deterministic() {
        let groups = vec![
            vec![1.0, 2.0, 3.0, 4.0, 5.0],
            vec![10.0, 11.0, 12.0, 13.0, 14.0],
        ];
        let (p, lo, hi) = stratified_bootstrap(&groups, mean, 500, 0.05, 1);
        assert!((p - 7.5).abs() < 1e-9);
        assert!(lo <= p && p <= hi);
        assert!(lo > 5.0 && hi < 10.0);
        assert_eq!(
            stratified_bootstrap(&groups, mean, 500, 0.05, 1),
            (p, lo, hi)
        );
    }
}
