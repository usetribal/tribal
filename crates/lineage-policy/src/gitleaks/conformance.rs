use serde::Deserialize;

use super::config::compile_config_from_str;
use super::detect::find_secret_spans_for_config;
use super::{apply_span_redactions, redact_text, REDACTED};

const FIXTURES: &str = include_str!("fixtures.json");

#[derive(Debug, Deserialize)]
struct FixtureCase {
    name: String,
    input: String,
    should_redact: bool,
    #[serde(default)]
    config: Option<String>,
}

#[test]
fn conformance_fixtures_match_expectations() {
    for case in load_fixtures() {
        let redacted = match case.config.as_deref() {
            Some(config) => {
                let compiled = compile_config_from_str(config);
                let spans = find_secret_spans_for_config(&compiled, &case.input);
                apply_span_redactions(&case.input, &spans)
            }
            None => redact_text(&case.input),
        };

        let did_redact = redacted.contains(REDACTED);
        assert_eq!(
            did_redact, case.should_redact,
            "fixture {:?}: input {:?}, redacted {:?}",
            case.name, case.input, redacted
        );
    }
}

fn load_fixtures() -> Vec<FixtureCase> {
    serde_json::from_str(FIXTURES).expect("fixtures.json must parse")
}

#[test]
fn default_config_parses_github_pat_allowlists_header() {
    let cfg = compile_config_from_str(include_str!("../../assets/gitleaks.toml"));
    let github = cfg
        .rules
        .iter()
        .find(|rule| rule.id == "github-pat")
        .expect("github-pat");
    assert!(!github.allowlist.paths.is_empty());
}
