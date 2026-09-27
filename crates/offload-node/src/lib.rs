//! The `offloadd` daemon.
//!
//! Phase 1 is one machine: no gossip, no bidding, no migration. What it does have is the
//! full run lifecycle — workspace, agent, event stream, cancellation — driving
//! `offload_core`'s real state machine, so phase 4 adds arbitration rather than replacing
//! this.

// Tests are allowed to panic loudly; the lint is about production paths.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod api;
pub mod approval_key;
pub mod asks;
pub mod broker;
pub mod client;
pub mod config;
pub mod daemon;
pub mod deliver;
pub mod explain;
pub mod fleet;
pub mod identity;
pub mod leftovers;
pub mod mesh;
pub mod render;
pub mod repo_config;
pub mod resource;
pub mod schedule;
pub mod server;
pub mod statedir;
pub mod supervisor;
pub mod task;
pub mod trigger;

pub use config::Config;
pub use supervisor::Supervisor;
