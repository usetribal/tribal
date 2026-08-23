//! Human-facing stdout is owned by [`ui`]. Command modules print through it;
//! `println!` and Debug formatting are denied here so a new command cannot invent
//! a second layout language.
#![deny(clippy::print_stdout, clippy::use_debug)]

pub mod auth;
pub mod brief;
pub mod commands;
pub mod context_cmd;
pub mod digest;
pub mod doctor_cmd;
pub mod events;
pub mod fork_cmd;
pub mod hooks_cmd;
pub mod init_cmd;
pub mod migrate;
pub mod progress;
pub mod pull_cmd;
pub mod repo_registry;
pub mod retrieval_cmd;
pub mod session_pick;
pub mod share_cmd;
pub mod share_fork;
pub mod skill_cmd;
pub mod ui;
pub mod update_check;
