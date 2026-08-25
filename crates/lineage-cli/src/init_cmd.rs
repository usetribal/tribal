//! Interactive `init` wizard. Box-drawing `println!` is wizard chrome only —
//! non-wizard lines go through [`crate::ui`].
#![allow(clippy::print_stdout)]

use std::path::Path;

use chrono::Utc;
use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};
use inquire::{Confirm, MultiSelect};

use crate::events::{EventLog, Outcome};
use crate::interactive::interactive;
use crate::ui;
use crate::{commands, context_cmd, hooks_cmd, skill_cmd};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SKILL_CHOICES: [(&str, &str, &str); 3] = [
    (".agents/skills/", "Open Standard / Codex", "codex"),
    (".claude/skills/", "Claude Code", "claude"),
    (".cursor/skills/", "Cursor", "cursor"),
];

/// Non-interactive / scripted init flags.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub yes: bool,
    pub targets: Vec<String>,
    pub no_skill: bool,
    pub no_import: bool,
    pub force_hooks: bool,
    pub steps: Steps,
}

/// Which individual setup steps were named. All false means the full setup —
/// the flags select a subset rather than adding one, so a new step becomes a
/// field here and nothing else has to learn about it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Steps {
    pub config: bool,
    pub skills: bool,
    pub hooks: bool,
    pub uninstall: bool,
}

impl Steps {
    fn any(&self) -> bool {
        self.config || self.skills || self.hooks || self.uninstall
    }
}

/// Map interactive menu selection to `init-skill` target strings.
pub fn parse_skill_selection(input: &str) -> Option<Vec<String>> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed == "0" || trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    if trimmed == "4" || trimmed.eq_ignore_ascii_case("all") {
        return Some(vec!["all".into()]);
    }

    let mut targets = Vec::new();
    for part in trimmed.split([',', ' ']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part {
            "1" | "agents" | "codex" => targets.push("codex".into()),
            "2" | "claude" => targets.push("claude".into()),
            "3" | "cursor" => targets.push("cursor".into()),
            "4" | "all" => return Some(vec!["all".into()]),
            "0" | "none" => return None,
            other => {
                return Some(vec![other.to_lowercase()]);
            }
        }
    }

    if targets.is_empty() {
        None
    } else {
        Some(targets)
    }
}

fn skill_targets_for_init(options: &InitOptions) -> Option<Vec<String>> {
    if options.no_skill {
        return None;
    }
    if options.targets.iter().any(|t| t == "none") {
        return None;
    }
    if !options.targets.is_empty() {
        return Some(options.targets.clone());
    }
    if options.yes {
        return Some(vec!["all".into()]);
    }
    None
}

pub fn init(repo_path: &Path, options: InitOptions) -> Result<()> {
    if options.steps.any() {
        return run_steps(repo_path, &options);
    }
    if options.yes {
        run_non_interactive(repo_path, &options)
    } else if !stdin_is_tty() {
        Err(
            "stdin is not a TTY; use --yes for non-interactive init (see: tribal init --help)"
                .into(),
        )
    } else {
        run_interactive(repo_path, &options)
    }
}

/// Run only the named steps. Each is the same call the full setup makes, so a
/// step cannot behave differently depending on how it was reached.
fn run_steps(repo_path: &Path, options: &InitOptions) -> Result<()> {
    if options.steps.uninstall {
        hooks_cmd::uninstall_hook(repo_path)?;
        let removed = context_cmd::uninstall_claude_agent_hook(repo_path)?;
        ui::action(format!(
            "context hook {}",
            if removed { "removed" } else { "not present" }
        ));
        return Ok(());
    }
    if options.steps.config {
        commands::init_config(repo_path)?;
    }
    if options.steps.skills {
        let targets = skill_targets_for_init(options).unwrap_or_else(|| vec!["all".into()]);
        skill_cmd::init_skill(repo_path, &targets, true)?;
    }
    if options.steps.hooks {
        install_hooks_with_retry(repo_path, options.force_hooks, false)?;
    }
    Ok(())
}

fn run_non_interactive(repo_path: &Path, options: &InitOptions) -> Result<()> {
    ui::heading("Lineage setup");
    ui::kv("Repository", repo_path.display());
    ui::blank();

    commands::init_config(repo_path)?;
    let skill_targets = skill_targets_for_init(options);
    match skill_targets {
        None => ui::action("agent skills: skipped"),
        Some(ref targets) => skill_cmd::init_skill(repo_path, targets, true)?,
    }
    let hooks_installed = install_hooks_with_retry(repo_path, options.force_hooks, false)?;
    install_claude_agent_hook_for_targets(repo_path, skill_targets.as_deref(), false)?;
    if !options.no_import {
        ui::action("running: tribal import --agent all --incremental");
        commands::import(repo_path, &["all".into()], None, true, true)?;
    } else {
        ui::action("initial import: skipped");
    }
    ui::blank();
    print_footer();
    log_init_event(
        repo_path,
        skill_targets.as_deref(),
        hooks_installed,
        !options.no_import,
    );
    Ok(())
}

fn log_init_event(
    repo_path: &Path,
    skill_targets: Option<&[String]>,
    hooks_installed: bool,
    import_run: bool,
) {
    let Some(log) = EventLog::for_repo_path(repo_path) else {
        return;
    };
    let targets: Vec<&str> = skill_targets
        .map(|t| {
            skill_cmd::resolve_targets(t)
                .into_iter()
                .map(|t| t.id())
                .collect()
        })
        .unwrap_or_default();
    log.append(
        Utc::now(),
        "init",
        Outcome::Ok,
        serde_json::json!({
            "targets": targets,
            "skill_installed": skill_targets.is_some(),
            "hooks_installed": hooks_installed,
            "import_run": import_run,
        }),
    );
}

fn run_interactive(repo_path: &Path, options: &InitOptions) -> Result<()> {
    print_header(repo_path);

    step_heading("Configure", None);
    commands::init_config_quiet(repo_path)?;
    step_item(true, "wrote default config to refs/lineage/config");
    step_item(
        false,
        "ensured .gitattributes for .lineage/media/** LFS pointers",
    );
    println!();

    step_heading(
        "Agent skills",
        Some("bundled lineage and share skills for your coding agents"),
    );
    let skill_targets = prompt_skill_targets()?;
    match skill_targets {
        None => step_item(false, "skipped"),
        Some(ref targets) => {
            skill_cmd::init_skill_quiet(repo_path, targets, true)?;
            let installed = skill_cmd::resolve_targets(targets);
            for (i, target) in installed.iter().enumerate() {
                let last = i + 1 == installed.len();
                step_item(!last, format!("{}", target.skills_dir(repo_path).display()));
            }
        }
    }
    println!();

    step_heading(
        "Git hooks",
        Some("pre-commit import and post-commit linking"),
    );
    let hooks_installed = install_hooks_with_retry(repo_path, options.force_hooks, true)?;
    println!();

    install_claude_agent_hook_for_targets(repo_path, skill_targets.as_deref(), true)?;

    step_heading(
        "Initial import",
        Some("import your agent transcripts into refs/lineage/*"),
    );
    let run_import = prompt_run_import()?;
    if run_import {
        step_item(true, "running import --agent all --incremental");
        commands::import(repo_path, &["all".into()], None, true, true)?;
        println!("└─ ✓ import finished");
    } else {
        step_item(false, "skipped");
    }
    println!();

    print_footer();
    log_init_event(
        repo_path,
        skill_targets.as_deref(),
        hooks_installed,
        run_import,
    );
    Ok(())
}

/// The agent hook follows the Claude skill choice: a repo whose engineer
/// skipped Claude tooling should not grow a `.claude/` directory.
fn install_claude_agent_hook_for_targets(
    repo_path: &Path,
    targets: Option<&[String]>,
    interactive: bool,
) -> Result<()> {
    let claude_selected = targets
        .is_some_and(|t| skill_cmd::resolve_targets(t).contains(&skill_cmd::SkillTarget::Claude));
    if !claude_selected {
        return Ok(());
    }

    let installed = context_cmd::install_claude_agent_hook(repo_path)?;
    let message = if installed {
        "context hook wired into .claude/settings.json (PostToolUse on Read)"
    } else {
        "context hook already present in .claude/settings.json"
    };
    if interactive {
        step_heading("Context hook", Some("inject provenance on file reads"));
        step_item(false, message);
        println!();
    } else {
        println!("{message}");
    }
    Ok(())
}

/// Returns whether the hooks were actually installed (a declined overwrite is
/// `Ok(false)`), so the `init` event can record the truth.
fn install_hooks_with_retry(repo_path: &Path, force: bool, quiet: bool) -> Result<bool> {
    let install = |force: bool| {
        if quiet {
            hooks_cmd::install_hook_quiet(repo_path, force)
        } else {
            hooks_cmd::install_hook(repo_path, force)
        }
    };

    match install(force) {
        Ok(()) => {
            if quiet {
                step_item(true, "pre-commit (incremental import)");
                step_item(false, "post-commit (link sessions to HEAD)");
            }
            Ok(true)
        }
        Err(e) if !force && stdin_is_tty() => {
            if quiet {
                step_item(true, format!("existing hooks detected: {e}"));
            } else {
                eprintln!("  {e}");
            }
            if confirm_prompt("Overwrite existing git hooks?", false)? {
                install(true)?;
                if quiet {
                    step_item(true, "pre-commit (incremental import)");
                    step_item(false, "post-commit (link sessions to HEAD)");
                }
                Ok(true)
            } else {
                if quiet {
                    step_item(false, "skipped hook install");
                } else {
                    println!("  skipped hook install");
                }
                Ok(false)
            }
        }
        Err(e) => Err(e),
    }
}

fn prompt_skill_targets() -> Result<Option<Vec<String>>> {
    let labels: Vec<String> = SKILL_CHOICES
        .iter()
        .map(|(path, desc, _)| format!("{path}  ({desc})"))
        .collect();

    // Prompt on its own line; empty message keeps options on the next line (inquire inlines them otherwise).
    println!("│ Install skill to:");
    let selected = MultiSelect::new("", labels)
        .with_all_selected_by_default()
        .without_filtering()
        .with_help_message("↑↓ move · space toggle · → select all · ← clear · enter confirm")
        .with_render_config(inquire_render_config())
        .prompt()?;

    if selected.is_empty() {
        return Ok(None);
    }

    let mut targets = Vec::new();
    for label in selected {
        let Some((_, _, target)) = SKILL_CHOICES
            .iter()
            .find(|(path, desc, _)| format!("{path}  ({desc})") == label)
        else {
            return Err(format!("unknown skill choice: {label}").into());
        };
        targets.push((*target).to_string());
    }

    Ok(Some(targets))
}

fn prompt_run_import() -> Result<bool> {
    Confirm::new("Import your agent transcripts now?")
        .with_default(true)
        .with_help_message(
            "Reads agent transcripts · writes refs/lineage/* · redacts secrets · safe to re-run",
        )
        .with_render_config(inquire_render_config())
        .prompt()
        .map_err(Into::into)
}

fn confirm_prompt(message: &str, default_yes: bool) -> Result<bool> {
    Confirm::new(message)
        .with_default(default_yes)
        .with_render_config(inquire_render_config())
        .prompt()
        .map_err(Into::into)
}

fn print_header(repo_path: &Path) {
    ui::banner();
    let repo_line = format!("Repository: {}", repo_path.display());
    println!();
    draw_box(&["Lineage setup", repo_line.as_str()], 36);
    println!();
}

/// Draw a aligned box: `│  content  │` rows with matching top/bottom borders.
fn draw_box(lines: &[&str], min_width: usize) {
    let content_width = lines
        .iter()
        .map(|line| line.len())
        .max()
        .unwrap_or(0)
        .max(min_width);
    let rule = "─".repeat(content_width + 3);

    println!("{}", ui::dim(format!("╭{rule}╮")));
    for (index, line) in lines.iter().enumerate() {
        let padded = format!("{line:<content_width$}");
        let body = if index == 0 {
            ui::accent(&padded)
        } else {
            padded
        };
        println!("{}  {body} {}", ui::dim("│"), ui::dim("│"));
    }
    println!("{}", ui::dim(format!("╰{rule}╯")));
}

fn step_heading(title: &str, detail: Option<&str>) {
    println!("{} {}", ui::dim("│"), ui::accent(title));
    if let Some(detail) = detail {
        println!("{}   {}", ui::dim("│"), ui::dim(detail));
    }
}

fn step_item(more_follows: bool, message: impl AsRef<str>) {
    let branch = if more_follows { "├" } else { "└" };
    println!(
        "{} {} {}",
        ui::dim(format!("{branch}─")),
        ui::ok("✓"),
        message.as_ref()
    );
}

fn print_footer() {
    draw_box(
        &[
            "Done",
            "tribal list",
            "tribal blame <file>:<line>",
            "tribal context query \"<question>\"",
        ],
        40,
    );
    println!();
}

pub(crate) fn inquire_render_config() -> RenderConfig<'static> {
    let mut cfg = RenderConfig::default();
    let accent = StyleSheet::new().with_fg(Color::LightCyan);
    cfg.prompt_prefix = Styled::new("│").with_fg(Color::DarkGrey);
    cfg.prompt = accent;
    cfg.help_message = StyleSheet::new().with_fg(Color::DarkGrey);
    cfg
}

fn stdin_is_tty() -> bool {
    interactive()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_selection_all() {
        assert_eq!(parse_skill_selection("4"), Some(vec!["all".into()]));
        assert_eq!(parse_skill_selection("all"), Some(vec!["all".into()]));
    }

    #[test]
    fn parse_skill_selection_none() {
        assert_eq!(parse_skill_selection("0"), None);
        assert_eq!(parse_skill_selection("none"), None);
    }

    #[test]
    fn parse_skill_selection_multiselect() {
        assert_eq!(
            parse_skill_selection("1,3"),
            Some(vec!["codex".into(), "cursor".into()])
        );
    }

    #[test]
    fn skill_targets_for_init_yes_defaults_all() {
        let opts = InitOptions {
            yes: true,
            ..Default::default()
        };
        assert_eq!(skill_targets_for_init(&opts), Some(vec!["all".into()]));
    }

    #[test]
    fn skill_targets_for_init_no_skill() {
        let opts = InitOptions {
            yes: true,
            no_skill: true,
            ..Default::default()
        };
        assert_eq!(skill_targets_for_init(&opts), None);
    }
}
