//! Rank Order Centroid (ROC) weights: turn "A matters more than B matters more than C" into
//! suggested numeric weights.
//!
//! For n ranked items, `ROC_k = (1/n) * Σ_{j=k..n} 1/j` (k = 1 is the most important). The values
//! sum to 1. Items that tie share the average of the ROC values for the positions they occupy.

/// Raw ROC values for positions 1..=n (index 0 is position 1).
pub fn roc_values(n: usize) -> Vec<f64> {
    (1..=n)
        .map(|k| (k..=n).map(|j| 1.0 / j as f64).sum::<f64>() / n as f64)
        .collect()
}

/// Suggested 1-10 weights from ranks.
///
/// `ranks[i]` is the rank of item i: smaller means more important, equal ranks tie. The numbers
/// only matter relative to each other (`[1, 2, 2, 5]` is the same as `[0, 1, 1, 2]`). The result is
/// scaled so the most important item gets 10, rounded, and never below 1.
pub fn roc_weights(ranks: &[u32]) -> Vec<u8> {
    let n = ranks.len();
    if n == 0 {
        return Vec::new();
    }
    let roc = roc_values(n);

    // Indices of the items in order of importance. Items with the same rank end up next to each
    // other and occupy a run of consecutive positions.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| ranks[i]);

    let mut raw = vec![0.0; n];
    let mut start = 0;
    while start < n {
        let rank = ranks[order[start]];
        let end = start
            + order[start..]
                .iter()
                .take_while(|&&i| ranks[i] == rank)
                .count();
        // Positions start..end share the average of their ROC values.
        let avg = roc[start..end].iter().sum::<f64>() / (end - start) as f64;
        for &i in &order[start..end] {
            raw[i] = avg;
        }
        start = end;
    }

    let top = raw.iter().copied().fold(0.0, f64::max);
    raw.iter()
        .map(|&v| ((v / top * 10.0).round() as u8).clamp(1, 10))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_all(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-12, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn roc_values_n1_to_n6() {
        // Hand-calculated from the formula.
        approx_all(&roc_values(1), &[1.0]);
        approx_all(&roc_values(2), &[3.0 / 4.0, 1.0 / 4.0]);
        approx_all(&roc_values(3), &[11.0 / 18.0, 5.0 / 18.0, 2.0 / 18.0]);
        approx_all(
            &roc_values(4),
            &[25.0 / 48.0, 13.0 / 48.0, 7.0 / 48.0, 3.0 / 48.0],
        );
        approx_all(
            &roc_values(5),
            &[
                137.0 / 300.0,
                77.0 / 300.0,
                47.0 / 300.0,
                27.0 / 300.0,
                12.0 / 300.0,
            ],
        );
        approx_all(
            &roc_values(6),
            &[
                49.0 / 120.0,
                29.0 / 120.0,
                19.0 / 120.0,
                37.0 / 360.0,
                22.0 / 360.0,
                10.0 / 360.0,
            ],
        );
        for n in 1..=6 {
            let sum: f64 = roc_values(n).iter().sum();
            assert!((sum - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn roc_weights_without_ties() {
        assert_eq!(roc_weights(&[]), Vec::<u8>::new());
        assert_eq!(roc_weights(&[1]), [10]);
        assert_eq!(roc_weights(&[1, 2]), [10, 3]); // 3.33
        assert_eq!(roc_weights(&[1, 2, 3]), [10, 5, 2]); // 4.55, 1.82
        assert_eq!(roc_weights(&[1, 2, 3, 4]), [10, 5, 3, 1]); // 5.2, 2.8, 1.2
        assert_eq!(roc_weights(&[1, 2, 3, 4, 5]), [10, 6, 3, 2, 1]); // 5.62, 3.43, 1.97, 0.88
        assert_eq!(roc_weights(&[1, 2, 3, 4, 5, 6]), [10, 6, 4, 3, 1, 1]); // 5.92, 3.88, 2.52, 1.50, 0.68
    }

    #[test]
    fn input_order_does_not_matter() {
        assert_eq!(roc_weights(&[3, 1, 2]), [2, 10, 5]);
    }

    #[test]
    fn ties_share_the_average() {
        // n = 2, both tied: each gets (3/4 + 1/4) / 2 = 1/2 -> both top -> 10.
        assert_eq!(roc_weights(&[1, 1]), [10, 10]);
        // n = 3, last two tied: (5/18 + 2/18)/2 = 3.5/18 vs 11/18 -> 3.18 -> 3.
        assert_eq!(roc_weights(&[1, 2, 2]), [10, 3, 3]);
        // n = 3, first two tied: (11/18 + 5/18)/2 = 8/18 top; 2/18 -> 2.5 -> rounds to 3.
        assert_eq!(roc_weights(&[1, 1, 2]), [10, 10, 3]);
        // n = 4, middle two tied: top 25/48; (13+7)/2/48 = 10/48 -> 4; 3/48 -> 1.2 -> 1.
        assert_eq!(roc_weights(&[1, 2, 2, 3]), [10, 4, 4, 1]);
        // n = 6, all tied: everyone gets 1/6 -> 10.
        assert_eq!(roc_weights(&[7; 6]), [10; 6]);
        // n = 5, ranks given as arbitrary numbers with a tie at the bottom.
        // positions: 137, 77, 47, (27+12)/2 = 19.5 over 300 -> 10, 5.62->6, 3.43->3, 1.42->1.
        assert_eq!(roc_weights(&[10, 20, 30, 40, 40]), [10, 6, 3, 1, 1]);
    }
}
