//! `tribal --discover` — the command surface as JSON, for an agent to read.
//!
//! Run through the built binary because the point of the flag is what a caller
//! actually receives on stdout, and because walking the parser is the behaviour
//! under test: a surface written out by hand would pass a test that asserted the
//! same hand-written text.

use std::process::Command;

use serde_json::Value;

fn discover() -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_tribal"))
        .arg("--discover")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "discover failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("discover must emit valid JSON")
}

fn command<'a>(surface: &'a Value, name: &str) -> &'a Value {
    surface["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in the discovered surface"))
}

/// Discovery runs without a repository: an agent may be deciding whether to use
/// lineage at all, from anywhere.
#[test]
fn discover_needs_no_repository_and_no_subcommand() {
    let surface = discover();
    assert_eq!(surface["name"], "tribal");
    assert!(surface["version"].is_string());
}

/// The whole point over `--help`: a hidden command is the one an agent is least
/// able to guess, so it is reported rather than concealed.
#[test]
fn hidden_commands_are_reported_and_marked() {
    let surface = discover();
    for name in ["export", "materialize", "remap", "gc", "push", "pull"] {
        assert_eq!(
            command(&surface, name)["hidden"],
            Value::Bool(true),
            "{name} should be present and marked hidden"
        );
    }
    assert_eq!(command(&surface, "fork")["hidden"], Value::Bool(false));
}

/// Every command carries the group its help lists it under, so one surface
/// answers both "what is there" and "what is worth reaching for first".
#[test]
fn commands_carry_their_help_group() {
    let surface = discover();
    assert_eq!(command(&surface, "init")["group"], "Setup");
    assert_eq!(command(&surface, "fork")["group"], "Sessions");
    assert_eq!(command(&surface, "sync")["group"], "Team");
    assert_eq!(command(&surface, "doctor")["group"], "Maintenance");
    // Off the front page, so it belongs to no listed group.
    assert_eq!(command(&surface, "gc")["group"], "advanced");
}

/// `upgrade` runs before every command already, so it is off the headline help.
/// It still has to be discoverable and runnable — demoting a command must not
/// be the same as retiring it, or an agent loses the one verb that repairs
/// state an older version wrote.
#[test]
fn upgrade_is_discoverable_but_off_the_front_page() {
    let surface = discover();
    let upgrade = command(&surface, "upgrade");
    assert_eq!(upgrade["group"], "advanced");
    assert_eq!(
        upgrade["hidden"],
        Value::Bool(false),
        "demoted, not hidden — it is still a verb a person may reach for"
    );
}

/// The switch a machine caller is told to use has to be on the surface it reads,
/// or the instruction and the binary disagree.
#[test]
fn the_headless_switch_is_global_and_discoverable() {
    let output = Command::new(env!("CARGO_BIN_EXE_tribal"))
        .args(["list", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--no-interactive"), "{help}");
}

/// An agent composing a call needs to know a value follows the flag, and that a
/// bare argument is expected — getting either wrong produces an invalid command.
#[test]
fn options_distinguish_flags_from_values_and_positionals() {
    let surface = discover();
    let doctor = command(&surface, "doctor");
    let options = doctor["options"].as_array().unwrap();

    let json = options.iter().find(|o| o["long"] == "json").unwrap();
    assert_eq!(json["takes_value"], Value::Bool(false), "--json is a flag");

    let section = options.iter().find(|o| o["long"] == "section").unwrap();
    assert_eq!(section["takes_value"], Value::Bool(true));
    assert_eq!(section["repeatable"], Value::Bool(true));

    let show = command(&surface, "show");
    let id = show["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == "session_id")
        .unwrap();
    assert_eq!(id["positional"], Value::Bool(true));
    assert_eq!(id["required"], Value::Bool(true));
}

/// Renamed commands keep their old names working, and an agent that learned one
/// from an older doc needs to see that it still resolves.
#[test]
fn aliases_are_reported() {
    let surface = discover();
    let aliases = command(&surface, "fork")["aliases"].as_array().unwrap();
    assert!(
        aliases.contains(&Value::String("continue".into())),
        "{aliases:?}"
    );
    assert!(
        aliases.contains(&Value::String("resume".into())),
        "{aliases:?}"
    );
}

/// `context` is a whole vocabulary behind one name; a surface that stopped at
/// the top level would hide the verbs an agent most needs.
#[test]
fn nested_subcommands_are_reported() {
    let surface = discover();
    let nested = command(&surface, "context")["subcommands"]
        .as_array()
        .unwrap();
    let names: Vec<&str> = nested.iter().filter_map(|c| c["name"].as_str()).collect();
    for verb in lineage_retrieval::VERBS {
        assert!(
            names.contains(&verb.cli),
            "context omits {}: {names:?}",
            verb.cli
        );
    }
}

/// The surface must describe the binary that emitted it, not a list that drifted
/// from it: every discovered command has to actually run.
#[test]
fn every_discovered_command_resolves() {
    let surface = discover();
    for entry in surface["commands"].as_array().unwrap() {
        let name = entry["name"].as_str().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_tribal"))
            .args([name, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{name} --help did not resolve");
    }
}
