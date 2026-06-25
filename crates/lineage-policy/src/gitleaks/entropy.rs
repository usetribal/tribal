/// Shannon entropy of `data` (bits per character), matching gitleaks' `shannonEntropy`.
pub fn shannon_entropy(data: &str) -> f64 {
    if data.is_empty() {
        return 0.0;
    }

    let mut counts = std::collections::HashMap::new();
    let len = data.chars().count() as f64;
    for ch in data.chars() {
        *counts.entry(ch).or_insert(0u32) += 1;
    }

    let inv = 1.0 / len;
    let mut entropy = 0.0;
    for &count in counts.values() {
        let freq = count as f64 * inv;
        entropy -= freq * freq.log2();
    }
    entropy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_has_zero_entropy() {
        assert_eq!(shannon_entropy(""), 0.0);
    }

    #[test]
    fn repeated_char_has_zero_entropy() {
        assert_eq!(shannon_entropy("aaaaaaaaaa"), 0.0);
    }

    #[test]
    fn randomish_string_has_higher_entropy() {
        assert!(shannon_entropy("K8vM2pQw9rT5xYzB3cF7j") > 3.0);
    }
}
