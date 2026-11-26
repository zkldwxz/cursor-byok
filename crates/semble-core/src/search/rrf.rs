//! Reciprocal Rank Fusion of independently ranked semantic and lexical candidates.

use std::collections::HashMap;

const RRF_K: f32 = 60.0;

pub(super) fn contribution(rank: usize, weight: f32) -> f32 {
    weight / (RRF_K + rank as f32 + 1.0)
}

pub fn fuse(semantic: &[usize], lexical: &[usize], alpha: f32) -> HashMap<usize, f32> {
    let mut scores = HashMap::new();
    for (rank, index) in semantic.iter().enumerate() {
        *scores.entry(*index).or_insert(0.0) += contribution(rank, alpha);
    }
    for (rank, index) in lexical.iter().enumerate() {
        *scores.entry(*index).or_insert(0.0) += contribution(rank, 1.0 - alpha);
    }
    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewards_items_present_in_both_lists() {
        let scores = fuse(&[1, 2], &[2, 3], 0.5);
        assert!(scores[&2] > scores[&1]);
        assert!(scores[&2] > scores[&3]);
    }
}
