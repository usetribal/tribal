use std::path::PathBuf;

use lineage_cli::{
    commands, context_cmd, digest, doctor_cmd, hooks_cmd, init_cmd, retrieval_cmd, skill_cmd,
};
use lineage_retrieval::{DEFAULT_AROUND_RADIUS, DEFAULT_TRAVERSAL_LIMIT};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "git-lineage",
    about = "Git-native provenance for AI coding agents",
    version
)]
struct Cli {
    /// Repository path (defaults to current directory)
    #[arg(long, global = true)]
    repo: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
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
    /// Interactive setup: config, skill, hooks, optional import
    Init {
        /// Non-interactive defaults (all skill targets, install hooks, run import)
        #[arg(long)]
        yes: bool,
        /// Skill targets: cursor, claude, codex, agents, all, none
        #[arg(long = "target", value_parser = skill_cmd::parse_skill_target)]
        target: Vec<String>,
        /// Skip agent skill install
        #[arg(long)]
        no_skill: bool,
        /// Skip initial import
        #[arg(long, alias = "no-ingest")]
        no_import: bool,
        /// Overwrite existing git hooks
        #[arg(long, alias = "force")]
        force_hooks: bool,
    },
    /// Write default refs/lineage/config
    InitConfig,
    /// Install bundled agent skill for lineage context retrieval
    InitSkill {
        /// Targets: cursor, claude, codex, agents (same as codex), all (default: all)
        #[arg(long = "target", value_parser = skill_cmd::parse_skill_target)]
        target: Vec<String>,
        #[arg(long)]
        force: bool,
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
    /// Show lineage for a file line
    Blame {
        /// Path with optional :line suffix (e.g. src/main.rs:42)
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Install git hooks for automatic lineage import
    InstallHook {
        #[arg(long)]
        force: bool,
    },
    /// Remove lineage git hooks
    UninstallHook,
    /// Internal hook commands (called by git hooks)
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
    Export {
        #[arg(long)]
        redact: bool,
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Sign in to a Lineage server (browser device flow)
    Login {
        /// Server base URL (e.g. https://api.uselineage.io/api)
        #[arg(long)]
        server: String,
    },
    /// Push redacted sessions to a Lineage server
    Sync {
        /// Server base URL; defaults to the server stored by `login`
        #[arg(long)]
        server: Option<String>,
        /// Bearer token; falls back to LINEAGE_TOKEN, then the stored login
        #[arg(long)]
        token: Option<String>,
        /// Git remote whose URL identifies the repo to the server
        #[arg(long, default_value = "origin")]
        remote: String,
    },
    /// Search indexed sessions
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
    /// Deprecated alias for `rebuild index`
    #[command(hide = true)]
    RebuildIndex,
    /// Link a session to a commit
    Link {
        session_id: String,
        commit_sha: String,
    },
    /// Materialize line objects for sessions at a commit
    Materialize {
        #[arg(long)]
        commit: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Remap lineage after rebase (re-materialize at HEAD)
    Remap,
    /// Git LFS object transport for large session content
    Lfs {
        #[command(subcommand)]
        action: LfsAction,
    },
    /// Delete an imported session (and optionally purge LFS blobs)
    Delete {
        session_id: String,
        #[arg(long)]
        purge_blobs: bool,
    },
    /// Purge orphan line objects and unreferenced LFS blobs
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

    let cli = Cli::parse();
    let repo_path = cli.repo.unwrap_or_else(|| PathBuf::from("."));

    let result = match cli.command {
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
        } => init_cmd::init(
            &repo_path,
            init_cmd::InitOptions {
                yes,
                targets: target,
                no_skill,
                no_import,
                force_hooks,
            },
        ),
        Commands::InitConfig => commands::init_config(&repo_path),
        Commands::InitSkill { target, force } => skill_cmd::init_skill(&repo_path, &target, force),
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
        Commands::Blame { target, json } => commands::blame(&repo_path, &target, json),
        Commands::InstallHook { force } => hooks_cmd::install_hook(&repo_path, force),
        Commands::UninstallHook => hooks_cmd::uninstall_hook(&repo_path),
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
        Commands::Login { server } => commands::login(&server),
        Commands::Sync {
            server,
            token,
            remote,
        } => commands::sync(&repo_path, server.as_deref(), token.as_deref(), &remote),
        Commands::Search { query } => commands::search(&repo_path, &query),
        Commands::Rebuild { target, embed } => match target {
            None => commands::rebuild(&repo_path, embed),
            Some(RebuildTarget::Index { embed }) => commands::rebuild_index(&repo_path, embed),
            Some(RebuildTarget::Embeddings) => commands::rebuild_embeddings(&repo_path),
        },
        Commands::RebuildIndex => commands::rebuild_index(&repo_path, false),
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
