//! Suggested weights from a ranking: turn "A matters more than B matters more than C" into
//! numbers on the 1-10 weight scale.
//!
//! We use **rank-sum** weights. For n ranked items, the item at rank k (k = 1 is the most
//! important) gets the raw value `n - k + 1`, so the raw values are n, n-1, ..., 1. They fall in
//! equal steps, which matches what most people mean by "a bit less important than the one above".
//!
//! An earlier version used Rank Order Centroid (ROC), which is popular in the decision-analysis
//! literature but very steep: seven criteria became 10, 6, 4, 3, 2, 1, 1, so the bottom half
//! barely counted and people were surprised. Rank-sum gives 10, 9, 7, 6, 4, 3, 1 for the same
//! ranking.
//!
//! Items that tie share the average of the raw values for the positions they occupy. The result
//! is scaled so the most important item gets 10, rounded, and never below 1 (the slider's 0 means
//! "ignore", which a ranking should never suggest).

/// Raw rank-sum values for positions 1..=n (index 0 is position 1): `n, n-1, ..., 1`.
pub fn rank_sum_values(n: usize) -> Vec<f64> {
    (1..=n).map(|k| (n - k + 1) as f64).collect()
}

/// Suggested 1-10 weights from ranks.
///
/// `ranks[i]` is the rank of item i: smaller means more important, equal ranks tie. The numbers
/// only matter relative to each other (`[1, 2, 2, 5]` is the same as `[0, 1, 1, 2]`). The result is
/// in the same order as `ranks`.
pub fn suggested_weights(ranks: &[u32]) -> Vec<u8> {
    let n = ranks.len();
    if n == 0 {
        return Vec::new();
    }
    let values = rank_sum_values(n);

    // Indices of the items in order of importance. Items with the same rank end up next to each
    // other and occupy a run of consecutive positions. `sort_by_key` is stable, so the input
    // order of tied items doesn't change the result.
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
        // Positions start..end share the average of their raw values.
        let avg = values[start..end].iter().sum::<f64>() / (end - start) as f64;
        for &i in &order[start..end] {
            raw[i] = avg;
        }
        start = end;
    }

    // The top item (or group) always has the largest raw value, so it maps to exactly 10.
    let top = raw.iter().copied().fold(0.0, f64::max);
    raw.iter()
        .map(|&v| ((v / top * 10.0).round() as u8).clamp(1, 10))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ranks 1..=n with no ties.
    fn strict(n: u32) -> Vec<u32> {
        (1..=n).collect()
    }

    #[test]
    fn raw_values() {
        assert_eq!(rank_sum_values(0), Vec::<f64>::new());
        assert_eq!(rank_sum_values(1), [1.0]);
        assert_eq!(rank_sum_values(4), [4.0, 3.0, 2.0, 1.0]);
    }

    #[test]
    fn weights_without_ties_n1_to_n7() {
        // Hand-checked: 10 * (n - k + 1) / n, rounded half away from zero.
        assert_eq!(suggested_weights(&[]), Vec::<u8>::new());
        assert_eq!(suggested_weights(&strict(1)), [10]);
        assert_eq!(suggested_weights(&strict(2)), [10, 5]);
        assert_eq!(suggested_weights(&strict(3)), [10, 7, 3]); // 6.67, 3.33
        assert_eq!(suggested_weights(&strict(4)), [10, 8, 5, 3]); // 7.5, 5, 2.5
        assert_eq!(suggested_weights(&strict(5)), [10, 8, 6, 4, 2]);
        assert_eq!(suggested_weights(&strict(6)), [10, 8, 7, 5, 3, 2]); // 8.33, 6.67, 5, 3.33, 1.67
        assert_eq!(suggested_weights(&strict(7)), [10, 9, 7, 6, 4, 3, 1]); // 8.57, 7.14, 5.71, 4.29, 2.86, 1.43
    }

    #[test]
    fn input_order_does_not_matter() {
        assert_eq!(suggested_weights(&[3, 1, 2]), [3, 10, 7]);
    }

    #[test]
    fn ties_share_the_average() {
        // n = 2, both tied: each gets (2 + 1) / 2 = 1.5, which is the top -> 10.
        assert_eq!(suggested_weights(&[1, 1]), [10, 10]);
        // n = 3, last two tied: 3 and (2 + 1) / 2 = 1.5 -> 10, 5, 5.
        assert_eq!(suggested_weights(&[1, 2, 2]), [10, 5, 5]);
        // n = 3, first two tied: (3 + 2) / 2 = 2.5 top; 1 / 2.5 * 10 = 4.
        assert_eq!(suggested_weights(&[1, 1, 2]), [10, 10, 4]);
        // n = 4, middle two tied: 4, (3 + 2) / 2 = 2.5 -> 6.25 -> 6, 1 -> 2.5 -> 3.
        assert_eq!(suggested_weights(&[1, 2, 2, 3]), [10, 6, 6, 3]);
        // n = 6, all tied: everyone shares the top -> 10.
        assert_eq!(suggested_weights(&[7; 6]), [10; 6]);
        // n = 5, arbitrary rank numbers with a tie at the bottom: 5, 4, 3, (2 + 1) / 2 = 1.5.
        assert_eq!(suggested_weights(&[10, 20, 30, 40, 40]), [10, 8, 6, 3, 3]);
    }

    #[test]
    fn many_items_never_drop_below_one() {
        // With 30 criteria (the maximum) the last raw value is 1/30 of the top: 0.33 rounds to 0,
        // so the minimum of 1 applies.
        let w = suggested_weights(&strict(30));
        assert_eq!(w[0], 10);
        assert_eq!(*w.last().unwrap(), 1);
    }

    mod properties {
        use super::super::*;
        use proptest::prelude::*;

        proptest! {
            /// Weights lie in 1..=10, the most important item gets 10, and a more important item
            /// (smaller rank) never gets a smaller weight than a less important one.
            #[test]
            fn weights_are_in_range_and_non_increasing_with_rank(
                ranks in prop::collection::vec(1u32..=10, 1..=30)
            ) {
                let w = suggested_weights(&ranks);
                prop_assert_eq!(w.len(), ranks.len());
                prop_assert!(w.iter().all(|x| (1..=10).contains(x)));
                let best = *ranks.iter().min().unwrap();
                for (i, &r) in ranks.iter().enumerate() {
                    if r == best {
                        prop_assert_eq!(w[i], 10);
                    }
                    for (j, &s) in ranks.iter().enumerate() {
                        if r < s {
                            prop_assert!(w[i] >= w[j], "rank {} got {} < rank {} got {}", r, w[i], s, w[j]);
                        }
                        if r == s {
                            prop_assert_eq!(w[i], w[j]);
                        }
                    }
                }
            }
        }
    }
}
