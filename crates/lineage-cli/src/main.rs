use std::path::PathBuf;

use chrono::Utc;
use lineage_cli::{
    commands, context_cmd, digest, doctor_cmd, fork_cmd, hooks_cmd, init_cmd, pull_cmd,
    repo_registry, retrieval_cmd, session_pick, share_cmd, skill_cmd,
};
use lineage_retrieval::{DEFAULT_AROUND_RADIUS, DEFAULT_TRAVERSAL_LIMIT};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// The command list, grouped. clap 4 has one heading for the whole subcommand
/// list rather than one per subcommand, so the grouping a 20-command surface
/// needs has to be written out here. Only the commands worth reaching for
/// first appear: the rest still run, and `--help` on any of them still works.
/// The commands worth reaching for first, in the order the help lists them.
///
/// Single-sourced: `--help` renders this, and `--discover` reports each
/// command's group from it, so a command cannot be grouped in one and missing
/// from the other. Commands absent here still run and still have `--help` —
/// they are the repair verbs and internal endpoints, surfaced by `--discover`
/// under the "advanced" group rather than crowding the front page.
const COMMAND_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Setup",
        &[(
            "init",
            "Set up lineage here: config, agent skills, hooks, first import",
        )],
    ),
    (
        "Sessions",
        &[
            ("import", "Import agent sessions into lineage refs"),
            ("list", "List imported sessions"),
            ("show", "Show a session"),
            (
                "fork",
                "Continue a session — reopens yours, writes out anyone else's",
            ),
            ("blame", "Show which sessions wrote a line"),
            ("context", "Retrieve context by intent, file, or line"),
        ],
    ),
    (
        "Team",
        &[
            ("login", "Sign in to a Lineage server"),
            (
                "sync",
                "Exchange sessions with the server (push, then pull)",
            ),
            ("share", "Share one session as a link anyone can open"),
        ],
    ),
    (
        "Maintenance",
        &[
            ("doctor", "Check lineage health in this repository"),
            ("rebuild", "Rebuild derived state from stored sessions"),
        ],
    ),
];

/// The grouped command list `--help` shows, built from [`COMMAND_GROUPS`].
fn command_help() -> String {
    let mut out = String::new();
    for (index, (group, commands)) in COMMAND_GROUPS.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(group);
        out.push_str(":\n");
        for (name, summary) in *commands {
            out.push_str(&format!("  {name:<12}  {summary}\n"));
        }
    }
    out.push('\n');
    out
}

/// The group a command is listed under, or `"advanced"` for one that is not on
/// the front page. Read by `--discover`, so an agent sees the same grouping a
/// human does plus everything hiding behind it.
fn group_of(command: &str) -> &'static str {
    for (group, commands) in COMMAND_GROUPS {
        if commands.iter().any(|(name, _)| *name == command) {
            return group;
        }
    }
    "advanced"
}

#[derive(Parser)]
#[command(
    name = "git-lineage",
    about = "Git-native provenance for AI coding agents",
    version,
    help_template = "\
{about}

{usage-heading} {usage}
{after-help}Options:
{options}",
    disable_help_subcommand = true
)]
struct Cli {
    /// Repository path (defaults to current directory)
    #[arg(long, global = true)]
    repo: Option<PathBuf>,

    /// Print the whole command surface as JSON, for an agent to read
    #[arg(long, exclusive = true)]
    discover: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Check repository lineage health: setup, capture, materialization, links, activity
    Doctor {
        #[arg(long)]
        json: bool,
        /// Limit output to a section (repeatable): setup, capture, materialization, links, activity
        #[arg(long = "section")]
        section: Vec<String>,
        /// How many event-log entries the activity section shows
        #[arg(long, default_value_t = doctor_cmd::DEFAULT_ACTIVITY_LIMIT)]
        activity_limit: usize,
    },
    /// Set up lineage in this repository: config, agent skills, hooks, first import
    ///
    /// Run with no flags this is the whole setup, interactively. Each step is
    /// also addressable on its own — `--config`, `--skills`, `--hooks` — for
    /// re-running one part without the rest; naming any of them runs only what
    /// was named. `--uninstall` removes what the hook steps installed.
    Init {
        /// Non-interactive defaults (all skill targets, install hooks, run import)
        #[arg(long)]
        yes: bool,
        /// Run only the config step: write default refs/lineage/config
        #[arg(long)]
        config: bool,
        /// Run only the skills step: install the bundled agent skills
        #[arg(long)]
        skills: bool,
        /// Run only the hooks step: install the pre-commit and post-commit hooks
        #[arg(long)]
        hooks: bool,
        /// Remove the git hooks and agent-hook wiring that setup installed
        #[arg(long)]
        uninstall: bool,
        /// Skill targets: cursor, claude, codex, agents, all, none
        #[arg(long = "target", value_parser = skill_cmd::parse_skill_target)]
        target: Vec<String>,
        /// Skip agent skills install
        #[arg(long)]
        no_skill: bool,
        /// Skip initial import
        #[arg(long, alias = "no-ingest")]
        no_import: bool,
        /// Overwrite existing git hooks
        #[arg(long, alias = "force")]
        force_hooks: bool,
    },
    /// Import agent sessions into git lineage refs
    #[command(alias = "ingest")]
    Import {
        #[arg(long, value_parser = parse_agent)]
        agent: Vec<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        no_link_head: bool,
        #[arg(long)]
        incremental: bool,
    },
    /// List imported sessions
    List {
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show a session by ID
    Show {
        session_id: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        hydrate_images: bool,
    },
    /// Continue a session in your agent
    ///
    /// One verb for both ways a session can be continued, because which one
    /// applies is a fact about the session, not a choice worth making. A
    /// session your harness still holds is reopened in place — nothing is
    /// written and it stays the same session. Any other, a teammate's
    /// included, is written out as a new session that is yours, carrying their
    /// context with theirs recorded as the ancestor. Which happened is printed.
    ///
    /// You get their context, not their tools. Tool activity is replayed as
    /// prose, so nothing hands you file handles or checkpoints that no longer
    /// exist.
    ///
    /// Given a share link instead of an id, this fetches that one session
    /// without a login, resolves where to land it — this repository, a checkout
    /// you already have, or a fresh clone — and opens it. Nothing is asked;
    /// each choice is printed. `--no-open` prints the command instead of
    /// running it, and `--into <dir>` overrides where it lands.
    ///
    /// `--brief` does something different: it writes nothing and prints a
    /// self-contained context block — whose session it was, the turns that
    /// carry the intent and the code changes, and the traversal commands — for
    /// starting a *subagent* on the session instead of continuing it here. The
    /// block ends with a marked slot for the subagent's task. It works for any
    /// stored session, including one pulled from a teammate that this build
    /// cannot write a transcript for.
    #[command(name = "fork", alias = "continue", alias = "resume")]
    Fork {
        /// Session to continue — lineage id, id prefix, harness UUID
        /// (`git lineage list` shows titles and ids), or a share link
        /// (`https://<host>/s/<token>`)
        session_id: Option<String>,
        /// Search local sessions by topic when no id is given
        #[arg(long)]
        query: Option<String>,
        /// Pick the Nth search result (1-based) when --query matches several
        #[arg(long)]
        pick: Option<usize>,
        /// Write the session out as a new one even if your harness holds it
        #[arg(long = "new", alias = "fork")]
        new_session: bool,
        /// Print a context block for a subagent instead of continuing the session
        #[arg(long)]
        brief: bool,
        /// Structured output for agents (candidates or resolved session)
        #[arg(long)]
        json: bool,
        /// Share links: continue into this directory instead of resolving where to land
        #[arg(long)]
        into: Option<PathBuf>,
        /// Share links: Lineage server to fetch from (default: derived from the link)
        #[arg(long)]
        server: Option<String>,
        /// Share links: print the command to continue the session instead of running it
        #[arg(long)]
        no_open: bool,
    },
    /// Show lineage for a file line
    Blame {
        /// Path with optional :line suffix (e.g. src/main.rs:42)
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Internal hook commands (called by git hooks)
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
    /// Context oracle: agent-hook injection endpoint and injection log
    Context {
        #[command(subcommand)]
        action: ContextAction,
    },
    /// Export sessions (optionally redacted)
    #[command(hide = true)]
    Export {
        #[arg(long)]
        redact: bool,
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Sign in to a Lineage server (browser device flow)
    Login {
        /// Server base URL; defaults to production or the server stored by a previous login
        #[arg(long)]
        server: Option<String>,
    },
    /// Push redacted sessions to a Lineage server
    #[command(name = "push", hide = true)]
    Push {
        /// Server base URL; defaults to production or the server stored by `login`
        #[arg(long)]
        server: Option<String>,
        /// Bearer token; falls back to LINEAGE_TOKEN, then the stored login
        #[arg(long)]
        token: Option<String>,
        /// Git remote whose URL identifies the repo to the server
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Exchange sessions with a Lineage server: push, then pull
    ///
    /// The two directions are not mirror images — a push merges into an
    /// authority, a pull merges into a local cache — so they stay separately
    /// addressable as `push` and `pull`. This runs both, in the order that
    /// leaves your work on the server before teammates' work arrives here.
    Sync {
        /// Server base URL; defaults to production or the server stored by `login`
        #[arg(long)]
        server: Option<String>,
        /// Bearer token; falls back to LINEAGE_TOKEN, then the stored login
        #[arg(long)]
        token: Option<String>,
        /// Git remote whose URL identifies the repo to the server
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Share one session as a link anyone can open without an account
    ///
    /// Pushes the session you are in to the Lineage server the same way `push`
    /// does — same redaction, and a private session is refused rather than
    /// stripped — then mints a link pinned at the turns it has now. Continuing
    /// the session afterwards does not change what the link shows.
    Share {
        /// Server base URL; defaults to production or the server stored by `login`
        #[arg(long)]
        server: Option<String>,
        /// Bearer token; falls back to LINEAGE_TOKEN, then the stored login
        #[arg(long)]
        token: Option<String>,
        /// Git remote whose URL identifies the repo to the server
        #[arg(long, default_value = "origin")]
        remote: String,
        /// Session to share — lineage id, id prefix, or harness UUID; defaults
        /// to the most recently active session for this directory
        #[arg(long)]
        session: Option<String>,
        /// Print the link without opening a browser
        #[arg(long)]
        no_open: bool,
    },
    /// Pull teammates' sessions down from a Lineage server
    ///
    /// Never deletes: sessions the server does not mention are left alone, and
    /// turns you already have are kept as they are.
    #[command(hide = true)]
    Pull {
        /// Server base URL; defaults to production or the server stored by `login`
        #[arg(long)]
        server: Option<String>,
        /// Bearer token; falls back to LINEAGE_TOKEN, then the stored login
        #[arg(long)]
        token: Option<String>,
        /// Git remote whose URL identifies the repo to the server
        #[arg(long, default_value = "origin")]
        remote: String,
        /// Report what would be pulled without writing anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Search indexed sessions (superseded by `context query`)
    #[command(hide = true)]
    Search { query: String },
    /// Rebuild derived state (links, line objects, index) from stored sessions
    Rebuild {
        #[command(subcommand)]
        target: Option<RebuildTarget>,
        /// Also run the dense-embedding backfill after the rebuild. Off by
        /// default — a full re-embed touches every session.
        #[arg(long)]
        embed: bool,
    },
    /// Link a session to a commit
    #[command(hide = true)]
    Link {
        session_id: String,
        commit_sha: String,
    },
    /// Materialize line objects for sessions at a commit
    #[command(hide = true)]
    Materialize {
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Remap lineage after rebase (re-materialize at HEAD)
    #[command(hide = true)]
    Remap,
    /// Git LFS object transport for large session content
    #[command(hide = true)]
    Lfs {
        #[command(subcommand)]
        action: LfsAction,
    },
    /// Delete an imported session (and optionally purge LFS blobs)
    #[command(hide = true)]
    Delete {
        session_id: String,
        #[arg(long)]
        purge_blobs: bool,
    },
    /// Purge orphan line objects and unreferenced LFS blobs
    #[command(hide = true)]
    Gc,
}

#[derive(Subcommand)]
enum LfsAction {
    /// Show referenced vs present LFS objects
    Status,
    /// Push LFS pointer and data refs to remote
    Push {
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Fetch missing LFS objects from remote
    Fetch {
        #[arg(long, default_value = "origin")]
        remote: String,
    },
}

#[derive(Subcommand)]
enum HookAction {
    /// Link all sessions to HEAD (post-commit hook)
    PostCommit,
}

#[derive(Subcommand)]
enum RebuildTarget {
    /// Rebuild only the search index
    Index {
        /// Also run the dense-embedding backfill afterward. Off by default — a
        /// full re-embed touches every session.
        #[arg(long)]
        embed: bool,
    },
    /// Rebuild only the dense embeddings
    Embeddings,
}

#[derive(Subcommand)]
enum ContextAction {
    /// Agent-hook endpoint: read a hook event on stdin, emit injection JSON
    Hook {
        /// Harness whose hook payload is on stdin: claude (PostToolUse/Read) or
        /// claude-session-start (SessionStart)
        harness: String,
    },
    /// Show recorded context injections, newest last
    Log {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Wire the context hook into Claude Code settings
    Install {
        /// Install user-level (~/.claude/settings.json) — covers every repo
        #[arg(long)]
        user: bool,
    },
    /// Remove lineage context-hook wiring from Claude Code settings
    Uninstall {
        #[arg(long)]
        user: bool,
    },
    /// Report the corpus's turn-salience breakdown (what indexing keeps/drops)
    Salience,
    /// Print the temporal chain for a line: <file>:<line>, one hop per row
    Chain {
        /// The line to chain, as <file>:<line> (e.g. README.md:40)
        target: String,
    },
    /// Search the text of specific sessions (one call, not N greps)
    SearchWithin {
        /// The text to match
        text: String,
        /// A session to search, repeatable. Accepts a digest handle
        /// (`session#turn`) as well as a bare session id
        #[arg(long = "session", required = true)]
        session: Vec<String>,
        #[arg(long, default_value_t = DEFAULT_TRAVERSAL_LIMIT)]
        limit: usize,
    },
    /// Read the turns immediately before and after a turn
    Around {
        /// The turn to read around, as a digest handle (`session#turn`) or a
        /// bare turn id
        handle: String,
        /// How many turns either side
        #[arg(long, default_value_t = DEFAULT_AROUND_RADIUS)]
        radius: u32,
        #[arg(long, default_value_t = DEFAULT_TRAVERSAL_LIMIT)]
        limit: usize,
    },
    /// List the code a turn produced (file:line ranges)
    ProducedBy {
        /// The turn, as a digest handle (`session#turn`) or a bare turn id
        handle: String,
        #[arg(long, default_value_t = DEFAULT_TRAVERSAL_LIMIT)]
        limit: usize,
    },
    /// Find the sessions behind a commit
    SessionsForCommit {
        /// Commit sha (short shas resolve as they do elsewhere in git)
        commit: String,
        #[arg(long, default_value_t = DEFAULT_TRAVERSAL_LIMIT)]
        limit: usize,
    },
    /// Retrieve past turns matching a free-text intent or a file[:line] anchor
    ///
    /// Precedence: --file forces the temporal plan; --lexical/--dense/--fused
    /// force one leg and skip the dispatcher; with none of those, the dispatcher
    /// routes the text (a named, existing file → temporal, else fused).
    Query {
        /// The intent / question to match; optional when --file anchors the query
        #[arg(default_value = "")]
        text: String,
        /// Line-anchored temporal plan: <path>[:<line>]. With text, the text
        /// re-ranks the anchored turns; alone, it returns them time-ordered.
        /// Forces temporal, skipping the dispatcher
        #[arg(long)]
        file: Option<String>,
        /// Lexical (FTS) leg only — skips the dispatcher
        #[arg(long)]
        lexical: bool,
        /// Dense (semantic) leg only — skips the dispatcher
        #[arg(long)]
        dense: bool,
        /// Fused lexical + dense (RRF) — forces fused, skipping the dispatcher
        #[arg(long)]
        fused: bool,
        /// Print per-stage plan timings (the fused and temporal plans only)
        #[arg(long)]
        timing: bool,
    },
}

/// The whole command surface as JSON, walked from the parser itself rather than
/// written out again — so it cannot drift from what the binary actually accepts,
/// and hidden commands are reported rather than concealed. An agent reading this
/// needs the repair verbs precisely because they are the ones it will not guess.
fn discover(command: &clap::Command) -> serde_json::Value {
    let commands: Vec<serde_json::Value> = command
        .get_subcommands()
        .map(|sub| {
            serde_json::json!({
                "name": sub.get_name(),
                "group": group_of(sub.get_name()),
                "hidden": sub.is_hide_set(),
                "summary": sub.get_about().map(|a| a.to_string()),
                "description": sub.get_long_about().map(|a| a.to_string()),
                "aliases": sub.get_all_aliases().collect::<Vec<_>>(),
                "options": options_of(sub),
                "subcommands": sub
                    .get_subcommands()
                    .map(|nested| serde_json::json!({
                        "name": nested.get_name(),
                        "summary": nested.get_about().map(|a| a.to_string()),
                        "options": options_of(nested),
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();

    serde_json::json!({
        "name": command.get_name(),
        "version": command.get_version(),
        "about": command.get_about().map(|a| a.to_string()),
        "groups": COMMAND_GROUPS.iter().map(|(g, _)| *g).collect::<Vec<_>>(),
        "commands": commands,
    })
}

/// Positionals and flags alike: an agent composing a call needs to know that
/// `show` takes a bare session id as much as it needs the flag names.
fn options_of(command: &clap::Command) -> Vec<serde_json::Value> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set())
        .map(|arg| {
            serde_json::json!({
                "name": arg.get_id().as_str(),
                "long": arg.get_long(),
                "positional": arg.is_positional(),
                "required": arg.is_required_set(),
                "repeatable": matches!(
                    arg.get_action(),
                    clap::ArgAction::Append | clap::ArgAction::Count
                ),
                "takes_value": !matches!(
                    arg.get_action(),
                    clap::ArgAction::SetTrue | clap::ArgAction::SetFalse | clap::ArgAction::Count
                ),
                "help": arg.get_help().map(|h| h.to_string()),
            })
        })
        .collect()
}

/// The parser with the grouped command list attached. `after_help` is built at
/// runtime from [`COMMAND_GROUPS`], so it cannot be a derive attribute.
fn cli_command() -> clap::Command {
    <Cli as clap::CommandFactory>::command().after_help(command_help())
}

fn parse_agent(s: &str) -> Result<String, String> {
    match s.to_lowercase().as_str() {
        "cursor" | "claude" | "codex" | "all" => Ok(s.to_lowercase()),
        other => Err(format!("unknown agent: {other}")),
    }
}

/// Map the query leg flags to a `Leg`, defaulting to fused (best quality).
/// All three legs are always available — the static embedder needs no build
/// opt-in — so this only validates that at most one flag is set.
fn select_leg(lexical: bool, dense: bool, fused: bool) -> Result<retrieval_cmd::Leg, String> {
    match (lexical, dense, fused) {
        (true, false, false) => Ok(retrieval_cmd::Leg::Lexical),
        (false, true, false) => Ok(retrieval_cmd::Leg::Dense),
        (false, false, true) => Ok(retrieval_cmd::Leg::Fused),
        // No leg flag: the dispatcher chooses the plan (temporal or fused). An
        // explicit `--fused` skips it, so the two cases stay distinct.
        (false, false, false) => Ok(retrieval_cmd::Leg::Default),
        _ => Err("choose at most one of --lexical / --dense / --fused".into()),
    }
}

/// The turn half of a digest handle. A bare id is already a turn id — the verbs
/// that take one are turn-addressed, so there is nothing else it could be.
fn turn_of(handle: &str) -> &str {
    let (session_id, turn_id) = digest::parse_handle(handle);
    turn_id.unwrap_or(session_id)
}

fn main() -> ExitCode {
    // Rust ignores SIGPIPE, so `doctor --json | head` would panic on EPIPE
    // mid-print. Restore die-on-SIGPIPE so piping behaves like any other CLI.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let cli = match cli_command().try_get_matches() {
        Ok(matches) => match <Cli as clap::FromArgMatches>::from_arg_matches(&matches) {
            Ok(cli) => cli,
            Err(error) => error.exit(),
        },
        Err(error) => error.exit(),
    };

    if cli.discover {
        let surface = discover(&cli_command());
        println!("{}", serde_json::to_string_pretty(&surface).unwrap());
        return ExitCode::SUCCESS;
    }

    // Only --discover may omit a subcommand; anything else with none is the
    // bare invocation, which should print help rather than silently succeed.
    let Some(command) = cli.command else {
        let _ = cli_command().print_help();
        return ExitCode::FAILURE;
    };

    let repo_path = cli.repo.unwrap_or_else(|| PathBuf::from("."));

    // The one place the registry is written. Recording here rather than in each
    // command means every subcommand run inside a checkout keeps it fresh, and
    // a new subcommand cannot forget to — which is what makes `fork <share-url>`
    // able to find a repository the receiver cloned months ago. Best-effort by
    // contract: a machine-level cache must never fail a repository command.
    repo_registry::record(&repo_path, Utc::now());

    let result = match command {
        Commands::Doctor {
            json,
            section,
            activity_limit,
        } => doctor_cmd::run(
            &repo_path,
            &doctor_cmd::DoctorArgs {
                json,
                sections: section,
                activity_limit,
            },
        ),
        Commands::Init {
            yes,
            target,
            no_skill,
            no_import,
            force_hooks,
            config,
            skills,
            hooks,
            uninstall,
        } => init_cmd::init(
            &repo_path,
            init_cmd::InitOptions {
                yes,
                targets: target,
                no_skill,
                no_import,
                force_hooks,
                steps: init_cmd::Steps {
                    config,
                    skills,
                    hooks,
                    uninstall,
                },
            },
        ),
        Commands::Import {
            agent,
            since,
            no_link_head,
            incremental,
        } => commands::import(
            &repo_path,
            &agent,
            since.as_deref(),
            !no_link_head,
            incremental,
        ),
        Commands::List { commit, json } => commands::list(&repo_path, commit.as_deref(), json),
        Commands::Show {
            session_id,
            json,
            hydrate_images,
        } => commands::show(&repo_path, &session_id, json, hydrate_images),
        Commands::Fork {
            session_id,
            query,
            pick,
            new_session,
            brief,
            json,
            into,
            server,
            no_open,
        } => fork_cmd::fork_session(
            &repo_path,
            fork_cmd::ForkRequest {
                pick: session_pick::ForkPickOptions {
                    session_id,
                    query,
                    pick,
                },
                force_fork: new_session,
                brief,
                json,
                share: fork_cmd::ShareOptions {
                    server,
                    into,
                    no_open,
                },
            },
        ),
        Commands::Blame { target, json } => commands::blame(&repo_path, &target, json),
        Commands::Hook { action } => match action {
            HookAction::PostCommit => hooks_cmd::post_commit(&repo_path),
        },
        Commands::Context { action } => match action {
            ContextAction::Hook { harness } => {
                // Fail open unconditionally: this runs inside an agent's tool
                // call, where a nonzero exit or stderr noise breaks a session
                // that context injection exists to help. An unknown harness is
                // silence, not an error.
                let endpoint = match harness.as_str() {
                    "claude" => Some(
                        context_cmd::hook_claude
                            as fn(&std::path::Path, &str, i64) -> Option<String>,
                    ),
                    "claude-session-start" => Some(
                        context_cmd::hook_claude_session_start
                            as fn(&std::path::Path, &str, i64) -> Option<String>,
                    ),
                    _ => None,
                };
                if let Some(endpoint) = endpoint {
                    let mut input = String::new();
                    use std::io::Read as _;
                    let _ = std::io::stdin().read_to_string(&mut input);
                    let now_unix = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if let Some(output) = endpoint(&repo_path, &input, now_unix) {
                        println!("{output}");
                    }
                }
                Ok(())
            }
            ContextAction::Log { limit } => context_cmd::print_log(&repo_path, limit),
            ContextAction::Install { user } => {
                let installed = if user {
                    context_cmd::install_claude_agent_hook_user()
                } else {
                    context_cmd::install_claude_agent_hook(&repo_path)
                };
                installed.map(|fresh| {
                    println!(
                        "context hook {}",
                        if fresh {
                            "installed"
                        } else {
                            "already present"
                        }
                    );
                })
            }
            ContextAction::Uninstall { user } => {
                let removed = if user {
                    context_cmd::uninstall_claude_agent_hook_user()
                } else {
                    context_cmd::uninstall_claude_agent_hook(&repo_path)
                };
                removed.map(|did| {
                    println!(
                        "context hook {}",
                        if did { "removed" } else { "not present" }
                    );
                })
            }
            ContextAction::Salience => retrieval_cmd::salience_report(&repo_path),
            ContextAction::Chain { target } => context_cmd::chain(&repo_path, &target),
            // Handles come back from a digest as `session#turn`; each verb takes
            // the half it addresses, so an agent can paste a handle unmodified.
            ContextAction::SearchWithin {
                text,
                session,
                limit,
            } => {
                let sessions: Vec<String> = session
                    .iter()
                    .map(|s| digest::parse_handle(s).0.to_string())
                    .collect();
                retrieval_cmd::search_within(&repo_path, &sessions, &text, limit)
            }
            ContextAction::Around {
                handle,
                radius,
                limit,
            } => retrieval_cmd::around(&repo_path, turn_of(&handle), radius, limit),
            ContextAction::ProducedBy { handle, limit } => {
                retrieval_cmd::produced_by(&repo_path, turn_of(&handle), limit)
            }
            ContextAction::SessionsForCommit { commit, limit } => {
                retrieval_cmd::sessions_for_commit_cmd(&repo_path, &commit, limit)
            }
            ContextAction::Query {
                text,
                file,
                lexical,
                dense,
                fused,
                timing,
            } => match select_leg(lexical, dense, fused) {
                Ok(leg) => retrieval_cmd::query(&repo_path, &text, leg, file.as_deref(), timing),
                Err(msg) => Err(msg.into()),
            },
        },
        Commands::Export { redact, format } => commands::export(&repo_path, redact, &format),
        Commands::Login { server } => commands::login(server.as_deref()),
        Commands::Push {
            server,
            token,
            remote,
        } => commands::sync(&repo_path, server.as_deref(), token.as_deref(), &remote),
        Commands::Sync {
            server,
            token,
            remote,
        } => commands::sync(&repo_path, server.as_deref(), token.as_deref(), &remote).and_then(
            |()| {
                pull_cmd::pull(
                    &repo_path,
                    server.as_deref(),
                    token.as_deref(),
                    &remote,
                    false,
                )
            },
        ),
        Commands::Share {
            server,
            token,
            remote,
            session,
            no_open,
        } => share_cmd::share(
            &repo_path,
            &share_cmd::ShareRequest {
                server,
                token,
                remote,
                session_id: session,
                no_open,
            },
        ),
        Commands::Pull {
            server,
            token,
            remote,
            dry_run,
        } => pull_cmd::pull(
            &repo_path,
            server.as_deref(),
            token.as_deref(),
            &remote,
            dry_run,
        ),
        Commands::Search { query } => commands::search(&repo_path, &query),
        Commands::Rebuild { target, embed } => match target {
            None => commands::rebuild(&repo_path, embed),
            Some(RebuildTarget::Index { embed }) => commands::rebuild_index(&repo_path, embed),
            Some(RebuildTarget::Embeddings) => commands::rebuild_embeddings(&repo_path),
        },
        Commands::Link {
            session_id,
            commit_sha,
        } => commands::link(&repo_path, &session_id, &commit_sha),
        Commands::Materialize { commit, session } => {
            commands::materialize(&repo_path, commit.as_deref(), session.as_deref())
        }
        Commands::Remap => commands::remap(&repo_path),
        Commands::Lfs { action } => match action {
            LfsAction::Status => commands::lfs_status_cmd(&repo_path),
            LfsAction::Push { remote } => commands::lfs_push_cmd(&repo_path, Some(&remote)),
            LfsAction::Fetch { remote } => commands::lfs_fetch_cmd(&repo_path, Some(&remote)),
        },
        Commands::Delete {
            session_id,
            purge_blobs,
        } => commands::delete_session_cmd(&repo_path, &session_id, purge_blobs),
        Commands::Gc => commands::gc_cmd(&repo_path),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
