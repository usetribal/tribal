use super::config::{compiled_config, CompiledConfig, CompiledRule, Span};
use super::entropy::shannon_entropy;

const GITLEAKS_ALLOW: &str = "gitleaks:allow";

/// Scan `text` with the embedded gitleaks rule set and return byte spans to redact.
pub fn find_secret_spans(text: &str) -> Vec<Span> {
    find_secret_spans_for_config(compiled_config(), text)
}

pub(crate) fn collect_keyword_hits(
    cfg: &CompiledConfig,
    normalized: &str,
) -> std::collections::HashSet<String> {
    let mut keyword_hits = std::collections::HashSet::new();
    let Some(trie) = cfg.keyword_trie.as_ref() else {
        return keyword_hits;
    };
    for mat in trie.find_iter(normalized) {
        keyword_hits.insert(normalized[mat.start()..mat.end()].to_string());
    }
    keyword_hits
}

pub(crate) fn find_secret_spans_for_config(cfg: &CompiledConfig, text: &str) -> Vec<Span> {
    let normalized = text.to_ascii_lowercase();
    let keyword_hits = collect_keyword_hits(cfg, &normalized);

    let mut spans = Vec::new();

    for rule in &cfg.rules {
        if !rule_keywords_present(rule, &normalized, &keyword_hits) {
            continue;
        }

        for caps in rule.regex.captures_iter(text) {
            let full_match = caps.get(0).expect("regex match must include group 0");
            let line = line_for_match(text, full_match.start(), full_match.end());
            if line.contains(GITLEAKS_ALLOW) {
                continue;
            }

            let secret = extract_secret(rule, &caps, full_match.as_str());
            if secret.is_empty() {
                continue;
            }

            if rule.entropy > 0.0 && shannon_entropy(secret) <= rule.entropy {
                continue;
            }

            if cfg.global_allowlist.allows_secret(secret) {
                continue;
            }
            if rule.allowlist.allows_secret(secret) {
                continue;
            }

            spans.push(Span {
                start: full_match.start(),
                end: full_match.end(),
            });
        }
    }

    merge_spans(&mut spans);
    spans
}

fn rule_keywords_present(
    rule: &CompiledRule,
    normalized: &str,
    keyword_hits: &std::collections::HashSet<String>,
) -> bool {
    if rule.keywords.is_empty() {
        return true;
    }
    rule.keywords
        .iter()
        .any(|kw| keyword_hits.contains(kw) || normalized.contains(kw.as_str()))
}

pub(crate) fn extract_secret<'a>(
    rule: &CompiledRule,
    caps: &regex::Captures<'a>,
    full_match: &'a str,
) -> &'a str {
    if caps.len() < 2 {
        return full_match.trim_matches('\n');
    }

    if rule.secret_group > 0 {
        if let Some(group) = caps.get(rule.secret_group) {
            return group.as_str();
        }
        return "";
    }

    for idx in 1..caps.len() {
        if let Some(group) = caps.get(idx) {
            let value = group.as_str();
            if value.is_empty() {
                continue;
            }
            return value;
        }
    }

    full_match.trim_matches('\n')
}

pub(crate) fn line_for_match(text: &str, start: usize, end: usize) -> &str {
    let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[end..]
        .find('\n')
        .map(|i| end + i)
        .unwrap_or(text.len());
    &text[line_start..line_end]
}

pub(crate) fn merge_spans(spans: &mut Vec<Span>) {
    if spans.is_empty() {
        return;
    }
    spans.sort_by_key(|s| s.start);
    let mut merged = vec![spans[0]];
    for span in spans.iter().skip(1) {
        let last = merged.last_mut().expect("merged non-empty");
        if span.start <= last.end {
            last.end = last.end.max(span.end);
        } else {
            merged.push(*span);
        }
    }
    *spans = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitleaks::config::{compile_config_from_str, CompiledAllowlist, CompiledConfig};

    fn cfg(src: &str) -> CompiledConfig {
        compile_config_from_str(src)
    }

    #[test]
    fn does_not_flag_innocent_prose() {
        let text = "The password: field in the schema is optional.\nsecret: my-aws-deploy-key";
        assert!(find_secret_spans(text).is_empty());
    }

    #[test]
    fn stripe_rule_compiles_and_detects() {
        let cfg = compiled_config();
        let stripe = cfg
            .rules
            .iter()
            .find(|rule| rule.id == "stripe-access-token")
            .expect("stripe rule should compile");
        let text = "export STRIPE_KEY=sk_test_abcdefghijklmnopqrstuvwxyz";
        assert!(stripe.regex.is_match(text));
        assert!(!find_secret_spans(text).is_empty());
    }

    #[test]
    fn flags_github_pat() {
        let text = "GITHUB_TOKEN=ghp_1234567890abcdefghijklmnopqrstuvwxyz12";
        assert!(!find_secret_spans(text).is_empty());
    }

    #[test]
    fn skips_line_marked_gitleaks_allow() {
        let text = "GITHUB_TOKEN=ghp_1234567890abcdefghijklmnopqrstuvwxyz12 gitleaks:allow";
        assert!(find_secret_spans(text).is_empty());
    }

    #[test]
    fn keywordless_rules_always_run() {
        let config = cfg(r#"
[[rules]]
id = "always"
regex = '''secret-([a-z]{4})'''
"#);
        let spans = find_secret_spans_for_config(&config, "value secret-abcd end");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, 6);
    }

    #[test]
    fn entropy_gate_rejects_low_entropy_secret() {
        let config = cfg(r#"
[[rules]]
id = "entropy"
regex = '''token=([a-z]+)'''
entropy = 4.0
keywords = ["token"]
"#);
        assert!(find_secret_spans_for_config(&config, "token=aaaa").is_empty());
        assert!(
            !find_secret_spans_for_config(&config, "token=abcdefghijklmnopqrstuvwxyz").is_empty()
        );
    }

    #[test]
    fn global_allowlist_suppresses_secret() {
        let config = cfg(r#"
[allowlist]
stopwords = ["allowlisted"]
[[rules]]
id = "token"
regex = '''token=([a-z]+)'''
keywords = ["token"]
"#);
        assert!(find_secret_spans_for_config(&config, "token=allowlisted").is_empty());
    }

    #[test]
    fn rule_allowlist_suppresses_secret() {
        let config = cfg(r#"
[[rules]]
id = "token"
regex = '''token=([a-z-]+)'''
keywords = ["token"]
[[rules.allowlists]]
stopwords = ["not-a-real-secret"]
"#);
        assert!(find_secret_spans_for_config(&config, "token=not-a-real-secret").is_empty());
    }

    #[test]
    fn secret_group_selects_expected_capture() {
        let config = cfg(r#"
[[rules]]
id = "grouped"
regex = '''prefix_([a-z]{3})_([a-z]{3})'''
secretGroup = 2
keywords = ["prefix"]
"#);
        let spans = find_secret_spans_for_config(&config, "value prefix_abc_def tail");
        assert_eq!(spans.len(), 1);
        let secret = extract_secret(
            &config.rules[0],
            &config.rules[0]
                .regex
                .captures("value prefix_abc_def tail")
                .expect("match"),
            "prefix_abc_def",
        );
        assert_eq!(secret, "def");
    }

    #[test]
    fn secret_group_out_of_range_yields_empty_secret() {
        let config = cfg(r#"
[[rules]]
id = "grouped"
regex = '''token=([a-z]+)'''
secretGroup = 3
keywords = ["token"]
"#);
        assert!(find_secret_spans_for_config(&config, "token=abcd").is_empty());
    }

    #[test]
    fn extract_secret_falls_back_to_first_nonempty_group() {
        let config = cfg(r#"
[[rules]]
id = "groups"
regex = '''token=([a-z]*)?(abcd)'''
"#);
        let caps = config.rules[0].regex.captures("token=abcd").expect("match");
        assert_eq!(
            extract_secret(&config.rules[0], &caps, caps.get(0).unwrap().as_str()),
            "abcd"
        );
    }

    #[test]
    fn extract_secret_trims_trailing_newline_without_groups() {
        let config = cfg(r#"
[[rules]]
id = "plain"
regex = '''SECRET'''
"#);
        let caps = config.rules[0].regex.captures("SECRET\n").expect("match");
        assert_eq!(
            extract_secret(&config.rules[0], &caps, "SECRET\n"),
            "SECRET"
        );
    }

    #[test]
    fn line_for_match_handles_first_and_last_line() {
        assert_eq!(line_for_match("only", 0, 4), "only");
        assert_eq!(line_for_match("a\nb\nc", 2, 3), "b");
        assert_eq!(line_for_match("a\nb\nc", 4, 5), "c");
    }

    #[test]
    fn merge_spans_combines_overlapping_ranges() {
        let mut spans = vec![
            Span { start: 10, end: 20 },
            Span { start: 15, end: 25 },
            Span { start: 30, end: 35 },
        ];
        merge_spans(&mut spans);
        assert_eq!(
            spans,
            vec![Span { start: 10, end: 25 }, Span { start: 30, end: 35 }]
        );
    }

    #[test]
    fn merge_spans_noop_for_empty_input() {
        let mut spans = Vec::new();
        merge_spans(&mut spans);
        assert!(spans.is_empty());
    }

    #[test]
    fn keyword_trie_records_hits_when_present() {
        let config = cfg(r#"
[[rules]]
id = "gated"
regex = '''sk_test_([a-z]+)'''
keywords = ["sk_test"]
"#);
        assert!(config.keyword_trie.is_some());
        let spans = find_secret_spans_for_config(&config, "prefix sk_test_abcdef suffix");
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn collect_keyword_hits_without_trie_returns_empty() {
        let cfg = CompiledConfig {
            global_allowlist: CompiledAllowlist::default(),
            rules: vec![],
            keyword_trie: None,
        };
        assert!(collect_keyword_hits(&cfg, "sk_test_value").is_empty());
    }

    #[test]
    fn collect_keyword_hits_records_trie_matches() {
        let config = cfg(r#"
[[rules]]
id = "gated"
regex = '''sk_test_([a-z]+)'''
keywords = ["sk_test"]
"#);
        let hits = collect_keyword_hits(&config, "prefix sk_test_suffix");
        assert!(hits.contains("sk_test"));
    }

    #[test]
    fn extract_secret_falls_back_when_optional_groups_are_missing() {
        let config = cfg(r#"
[[rules]]
id = "optional"
regex = '''token=(x)?(y)?'''
"#);
        let caps = config.rules[0].regex.captures("token=").expect("match");
        assert_eq!(extract_secret(&config.rules[0], &caps, "token="), "token=");
    }

    #[test]
    fn extract_secret_returns_first_nonempty_group_after_empty() {
        let config = cfg(r#"
[[rules]]
id = "groups"
regex = '''token=(|a)(value)'''
"#);
        let caps = config.rules[0]
            .regex
            .captures("token=value")
            .expect("match");
        assert_eq!(
            extract_secret(&config.rules[0], &caps, "token=value"),
            "value"
        );
    }

    #[test]
    fn extract_secret_skips_empty_capture_groups() {
        let config = cfg(r#"
[[rules]]
id = "empty-groups"
regex = '''token=(|a)(|b)'''
"#);
        let caps = config.rules[0].regex.captures("token=").expect("match");
        assert_eq!(extract_secret(&config.rules[0], &caps, "token="), "token=");
    }

    #[test]
    fn extract_secret_uses_full_match_when_groups_are_empty() {
        let config = cfg(r#"
[[rules]]
id = "empty-groups"
regex = '''token=( *)( *)'''
"#);
        let caps = config.rules[0].regex.captures("token=").expect("match");
        assert_eq!(extract_secret(&config.rules[0], &caps, "token="), "token=");
    }

    #[test]
    fn keyword_trie_hits_used_for_gated_rules() {
        let config = cfg(r#"
[[rules]]
id = "gated"
regex = '''fixturekey_([a-z]+)'''
keywords = ["fixturekey"]
"#);
        assert!(!find_secret_spans_for_config(&config, "fixturekey_abcdefghij").is_empty());
    }
}
