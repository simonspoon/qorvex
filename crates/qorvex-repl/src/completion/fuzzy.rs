//! Fuzzy matching for completion candidates.

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

/// Fuzzy filter for matching completion candidates.
pub struct FuzzyFilter {
    matcher: SkimMatcherV2,
}

impl Default for FuzzyFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzyFilter {
    /// Create a new fuzzy filter.
    pub fn new() -> Self {
        Self {
            matcher: SkimMatcherV2::default(),
        }
    }

    /// Score a target string against a query.
    ///
    /// Returns the score and matched character indices, or None if no match.
    /// Scoring: base fuzzy score + prefix bonus (+1000) + exact match bonus (+2000)
    pub fn score(&self, query: &str, target: &str) -> Option<(i64, Vec<usize>)> {
        if query.is_empty() {
            // Empty query matches everything with neutral score
            return Some((0, Vec::new()));
        }

        let query_lower = query.to_lowercase();
        let target_lower = target.to_lowercase();

        // Get base fuzzy match
        let (score, indices) = self.matcher.fuzzy_indices(&target_lower, &query_lower)?;

        // Apply bonuses
        let mut final_score = score;

        // Exact match bonus
        if target_lower == query_lower {
            final_score += 2000;
        }
        // Prefix match bonus
        else if target_lower.starts_with(&query_lower) {
            final_score += 1000;
        }

        Some((final_score, indices))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query_matches_all() {
        let filter = FuzzyFilter::new();
        assert!(filter.score("", "anything").is_some());
    }

    #[test]
    fn test_exact_match_highest_score() {
        let filter = FuzzyFilter::new();
        let exact = filter.score("tap_element", "tap_element").unwrap().0;
        let prefix = filter.score("tap", "tap_element").unwrap().0;
        let fuzzy = filter.score("taele", "tap_element").unwrap().0;

        assert!(exact > prefix);
        assert!(prefix > fuzzy);
    }

    #[test]
    fn test_case_insensitive() {
        let filter = FuzzyFilter::new();
        assert!(filter.score("TAP", "tap_element").is_some());
        assert!(filter.score("Tap", "TAP_ELEMENT").is_some());
    }

    #[test]
    fn test_fuzzy_match() {
        let filter = FuzzyFilter::new();
        // "taele" should match "tap_element"
        let result = filter.score("taele", "tap_element");
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match() {
        let filter = FuzzyFilter::new();
        assert!(filter.score("xyz", "tap_element").is_none());
    }
}
