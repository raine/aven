pub(crate) fn is_near(a: &str, b: &str) -> bool {
    is_contained_near(a, b) || levenshtein(a, b) <= 2
}

fn is_contained_near(a: &str, b: &str) -> bool {
    let shorter = a.len().min(b.len());
    let longer = a.len().max(b.len());
    shorter * 2 >= longer && (a.contains(b) || b.contains(a))
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut costs: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = i;
        costs[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let old = costs[j + 1];
            costs[j + 1] = if ca == cb {
                prev
            } else {
                1 + prev.min(costs[j]).min(costs[j + 1])
            };
            prev = old;
        }
    }
    costs[b.len()]
}

#[cfg(test)]
mod tests {
    use super::is_near;

    #[test]
    fn matches_close_typos_and_normalized_compounds() {
        assert!(is_near("home-lab", "homelab"));
        assert!(is_near("service-worker", "service-wroker"));
    }

    #[test]
    fn ignores_short_shared_suffixes() {
        assert!(!is_near(
            "regional-billing-service-worker",
            "service-worker"
        ));
        assert!(!is_near(
            "regional-billing-service-worker",
            "core-service-worker"
        ));
    }
}
