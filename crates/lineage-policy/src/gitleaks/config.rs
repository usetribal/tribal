use std::sync::OnceLock;

use aho_corasick::AhoCorasick;
use regex::Regex;

const DEFAULT_CONFIG: &str = include_str!("../../assets/gitleaks.toml");

static CONFIG: OnceLock<CompiledConfig> = OnceLock::new();

pub fn compiled_config() -> &'static CompiledConfig {
    CONFIG.get_or_init(|| compile_config_from_str(DEFAULT_CONFIG))
}

pub(crate) fn compile_config_from_str(src: &str) -> CompiledConfig {
    CompiledConfig::from_raw(parse_config(src))
}

#[derive(Debug, Default)]
pub(crate) struct RawConfig {
    allowlist: RawAllowlist,
    rules: Vec<RawRule>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct RawAllowlist {
    paths: Vec<String>,
    regexes: Vec<String>,
    stopwords: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct RawRule {
    id: String,
    regex: String,
    entropy: f64,
    keywords: Vec<String>,
    secret_group: usize,
    allowlist: RawAllowlist,
}

pub(crate) fn parse_config(src: &str) -> RawConfig {
    let mut config = RawConfig::default();
    if let Some(section) = extract_table_section(src, "[allowlist]") {
        config.allowlist = parse_allowlist_table(&section);
    }

    for rule_section in src.split("[[rules]]").skip(1) {
        let rule_end = rule_section
            .find("\n[[rules]]")
            .unwrap_or(rule_section.len());
        let rule_text = &rule_section[..rule_end];
        if let Some(rule) = parse_rule_block(rule_text) {
            config.rules.push(rule);
        }
    }

    config
}

fn extract_table_section(src: &str, header: &str) -> Option<String> {
    let start = src.find(header)? + header.len();
    let rest = &src[start..];
    let end = rest
        .find("\n[[rules]]")
        .or_else(|| rest.find("\n[rules."))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn parse_allowlist_table(section: &str) -> RawAllowlist {
    RawAllowlist {
        paths: parse_string_array(section, "paths"),
        regexes: parse_string_array(section, "regexes"),
        stopwords: parse_string_array(section, "stopwords"),
    }
}

fn parse_rule_block(section: &str) -> Option<RawRule> {
    let id = parse_quoted_field(section, "id")?;
    let regex = parse_triple_quoted_field(section, "regex")?;
    if regex.is_empty() {
        return None;
    }

    Some(RawRule {
        id,
        regex,
        entropy: parse_number_field(section, "entropy").unwrap_or(0.0),
        keywords: parse_string_array(section, "keywords"),
        secret_group: parse_number_field(section, "secretGroup")
            .map(|n| n as usize)
            .unwrap_or(0),
        allowlist: parse_rule_allowlists(section),
    })
}

fn parse_rule_allowlists(section: &str) -> RawAllowlist {
    let mut merged = RawAllowlist::default();
    for header in ["[[rules.allowlists]]", "[[rules.allowlist]]"] {
        let mut search = section;
        while let Some(start) = search.find(header) {
            let after = &search[start + header.len()..];
            let end = after
                .find("\n[[rules.")
                .or_else(|| after.find("\n[[rules]]"))
                .unwrap_or(after.len());
            let block = parse_allowlist_table(&after[..end]);
            merged.paths.extend(block.paths);
            merged.regexes.extend(block.regexes);
            merged.stopwords.extend(block.stopwords);
            search = &after[end..];
        }
    }
    merged
}

fn parse_quoted_field(section: &str, key: &str) -> Option<String> {
    let pattern = format!(r#"(?m)^\s*{key}\s*=\s*"((?:\\.|[^"\\])*)""#);
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(section)?;
    caps.get(1).map(|m| unescape_basic_string(m.as_str()))
}

fn parse_triple_quoted_field(section: &str, key: &str) -> Option<String> {
    let pattern = format!(r#"(?m)^\s*{key}\s*=\s*'''(.*)'''\s*$"#);
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(section)?;
    caps.get(1).map(|m| m.as_str().to_string())
}

fn parse_number_field(section: &str, key: &str) -> Option<f64> {
    let pattern = format!(r#"(?m)^\s*{key}\s*=\s*([0-9]+(?:\.[0-9]+)?)\s*$"#);
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(section)?;
    caps.get(1).and_then(|m| m.as_str().parse().ok())
}

fn parse_string_array(section: &str, key: &str) -> Vec<String> {
    let pattern = format!(r"(?ms)^\s*{key}\s*=\s*\[(.*?)\]");
    let Some(re) = Regex::new(&pattern).ok() else {
        return Vec::new();
    };
    let Some(caps) = re.captures(section) else {
        return Vec::new();
    };
    parse_array_values(caps.get(1).map(|m| m.as_str()).unwrap_or_default())
}

fn parse_array_values(section: &str) -> Vec<String> {
    let mut values = parse_quoted_strings(section);
    let triple = Regex::new(r"'''([^']*)'''").expect("triple quoted array regex");
    values.extend(
        triple
            .captures_iter(section)
            .map(|caps| caps.get(1).expect("group").as_str().to_string()),
    );
    values
}

fn parse_quoted_strings(section: &str) -> Vec<String> {
    let re = Regex::new(r#""((?:\\.|[^"\\])*)""#).expect("quoted string regex");
    re.captures_iter(section)
        .map(|caps| unescape_basic_string(caps.get(1).expect("group").as_str()))
        .collect()
}

fn unescape_basic_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

impl CompiledConfig {
    pub(crate) fn from_raw(raw: RawConfig) -> Self {
        let global_allowlist = CompiledAllowlist::compile(&raw.allowlist);

        let mut rules = Vec::new();
        let mut all_keywords = Vec::new();

        for rule in raw.rules {
            let Ok(regex) = Regex::new(&rule.regex) else {
                continue;
            };

            for kw in &rule.keywords {
                all_keywords.push(kw.to_ascii_lowercase());
            }

            rules.push(CompiledRule {
                id: rule.id,
                regex,
                entropy: rule.entropy,
                keywords: rule
                    .keywords
                    .into_iter()
                    .map(|k| k.to_ascii_lowercase())
                    .collect(),
                secret_group: rule.secret_group,
                allowlist: CompiledAllowlist::compile(&rule.allowlist),
            });
        }

        all_keywords.sort();
        all_keywords.dedup();

        let keyword_trie = AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(&all_keywords)
            .ok();

        Self {
            global_allowlist,
            rules,
            keyword_trie,
        }
    }
}

#[derive(Debug)]
pub struct CompiledConfig {
    pub global_allowlist: CompiledAllowlist,
    pub rules: Vec<CompiledRule>,
    pub keyword_trie: Option<AhoCorasick>,
}

#[derive(Debug, Default, Clone)]
pub struct CompiledAllowlist {
    #[allow(dead_code)]
    pub paths: Vec<String>,
    pub regexes: Vec<Regex>,
    pub stopwords: Vec<String>,
}

impl CompiledAllowlist {
    pub(crate) fn compile(raw: &RawAllowlist) -> Self {
        Self {
            paths: raw.paths.clone(),
            regexes: raw
                .regexes
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            stopwords: raw.stopwords.clone(),
        }
    }

    pub fn allows_secret(&self, secret: &str) -> bool {
        if self.regexes.iter().any(|re| re.is_match(secret)) {
            return true;
        }
        self.stopwords
            .iter()
            .any(|word| word.eq_ignore_ascii_case(secret))
    }
}

#[derive(Debug)]
pub struct CompiledRule {
    #[allow(dead_code)]
    pub id: String,
    pub regex: Regex,
    pub entropy: f64,
    pub keywords: Vec<String>,
    pub secret_group: usize,
    pub allowlist: CompiledAllowlist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_default_gitleaks_rules() {
        let cfg = compiled_config();
        assert!(cfg.rules.len() > 100);
    }

    #[test]
    fn parses_stripe_rule_from_config() {
        let raw = parse_config(DEFAULT_CONFIG);
        let stripe = raw
            .rules
            .iter()
            .find(|rule| rule.id == "stripe-access-token")
            .expect("stripe rule present");
        assert!(!stripe.keywords.is_empty());
        assert!(stripe.regex.contains("sk|rk"));
    }

    #[test]
    fn parses_rules_allowlists_plural_header() {
        let src = r#"
[[rules]]
id = "demo"
regex = '''token=([a-z]+)'''
[[rules.allowlists]]
stopwords = ["demo-secret"]
regexes = ["^demo$"]
"#;
        let raw = parse_config(src);
        let rule = &raw.rules[0];
        assert_eq!(rule.allowlist.stopwords, vec!["demo-secret".to_string()]);
        assert_eq!(rule.allowlist.regexes, vec!["^demo$".to_string()]);
    }

    #[test]
    fn merges_multiple_rule_allowlists() {
        let src = r#"
[[rules]]
id = "demo"
regex = '''token=([a-z]+)'''
[[rules.allowlists]]
stopwords = ["one"]
[[rules.allowlists]]
stopwords = ["two"]
"#;
        let raw = parse_config(src);
        assert_eq!(
            raw.rules[0].allowlist.stopwords,
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn parses_global_allowlist_section() {
        let src = r#"
[allowlist]
stopwords = ["abc"]
regexes = ["^zzz$"]
[[rules]]
id = "demo"
regex = '''x'''
"#;
        let raw = parse_config(src);
        assert_eq!(raw.allowlist.stopwords, vec!["abc".to_string()]);
        assert_eq!(raw.allowlist.regexes, vec!["^zzz$".to_string()]);
    }

    #[test]
    fn skips_rules_without_id_or_regex() {
        let src = r#"
[[rules]]
regex = '''token=([a-z]+)'''
[[rules]]
id = "empty"
regex = ''''''
"#;
        assert!(parse_config(src).rules.is_empty());
    }

    #[test]
    fn unescape_basic_string_covers_escape_branches() {
        assert_eq!(unescape_basic_string(r"line\nbreak"), "line\nbreak");
        assert_eq!(unescape_basic_string(r"tab\there"), "tab\there");
        assert_eq!(
            unescape_basic_string(r"return\rcarriage"),
            "return\rcarriage"
        );
        assert_eq!(unescape_basic_string(r"slash\\path"), "slash\\path");
        assert_eq!(unescape_basic_string(r#"quote\"ok"#), "quote\"ok");
        assert_eq!(unescape_basic_string(r"unknown\q"), r"unknown\q");
        assert_eq!(unescape_basic_string(r"trailing\\"), "trailing\\");
    }

    #[test]
    fn allows_secret_matches_regex_and_stopword() {
        let compiled = CompiledAllowlist::compile(&RawAllowlist {
            paths: vec![],
            regexes: vec!["^demo$".into()],
            stopwords: vec!["stop-me".into()],
        });
        assert!(compiled.allows_secret("demo"));
        assert!(compiled.allows_secret("stop-me"));
        assert!(!compiled.allows_secret("real-secret"));
    }

    #[test]
    fn compile_skips_invalid_allowlist_regex() {
        let compiled = CompiledAllowlist::compile(&RawAllowlist {
            paths: vec![],
            regexes: vec!["(unclosed".into(), "^ok$".into()],
            stopwords: vec![],
        });
        assert_eq!(compiled.regexes.len(), 1);
    }

    #[test]
    fn extract_table_section_ends_at_rules_dot_header() {
        let src = r#"
[allowlist]
stopwords = ["x"]
[rules.allowlists]
paths = ["noop"]
[[rules]]
id = "demo"
regex = '''y'''
"#;
        let raw = parse_config(src);
        assert_eq!(raw.allowlist.stopwords, vec!["x".to_string()]);
    }

    #[test]
    fn from_raw_skips_invalid_rule_regex() {
        let raw = RawConfig {
            rules: vec![
                RawRule {
                    id: "bad".into(),
                    regex: "(unclosed".into(),
                    ..RawRule::default()
                },
                RawRule {
                    id: "good".into(),
                    regex: "SECRET".into(),
                    ..RawRule::default()
                },
            ],
            ..RawConfig::default()
        };
        let compiled = CompiledConfig::from_raw(raw);
        assert_eq!(compiled.rules.len(), 1);
        assert_eq!(compiled.rules[0].id, "good");
    }

    #[test]
    fn parser_edge_cases_cover_error_paths() {
        assert!(extract_table_section("no allowlist", "[allowlist]").is_none());
        assert!(parse_quoted_field("id = \"x\"", "(?").is_none());
        assert!(parse_quoted_field("id = \"unclosed", "id").is_none());
        assert!(parse_triple_quoted_field("regex = '''open", "regex").is_none());
        assert!(parse_triple_quoted_field("x", "(?").is_none());
        assert!(parse_triple_quoted_field("missing", "regex").is_none());
        assert_eq!(parse_number_field("entropy = 3.5", "entropy"), Some(3.5));
        assert!(parse_number_field("entropy = not_a_number", "entropy").is_none());
        assert!(parse_number_field("missing", "entropy").is_none());
        assert!(parse_number_field("x", "(?").is_none());
        assert!(parse_string_array("text", "(?").is_empty());
        assert!(parse_string_array("text", "keywords").is_empty());
        assert_eq!(unescape_basic_string("\\"), "\\");
        assert_eq!(unescape_basic_string("end\\"), "end\\");
    }

    #[test]
    fn extract_table_section_ends_at_rules_dot_without_double_bracket() {
        let section = extract_table_section(
            "[allowlist]\nstopwords = [\"a\"]\n[rules.foo]\nrest = 1\n",
            "[allowlist]",
        )
        .expect("section");
        assert!(section.contains("stopwords"));
        assert!(!section.contains("rules.foo"));
    }

    #[test]
    fn from_raw_without_keywords_still_detects_secrets() {
        let compiled = CompiledConfig::from_raw(RawConfig {
            rules: vec![RawRule {
                id: "plain".into(),
                regex: "SECRET".into(),
                ..RawRule::default()
            }],
            ..RawConfig::default()
        });
        assert_eq!(compiled.rules.len(), 1);
        let spans = crate::gitleaks::detect::find_secret_spans_for_config(
            &compiled,
            "prefix SECRET suffix",
        );
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn parse_array_values_supports_triple_quoted_entries() {
        let values = parse_array_values("'''one''', '''two'''");
        assert_eq!(values, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn github_pat_rule_parses_allowlist_paths() {
        let raw = parse_config(DEFAULT_CONFIG);
        let github = raw
            .rules
            .iter()
            .find(|rule| rule.id == "github-pat")
            .expect("github-pat rule");
        assert!(!github.allowlist.paths.is_empty());
    }
}
