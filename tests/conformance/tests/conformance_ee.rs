//! The conformance scenarios checked against GitLab EE recordings (`fixtures-ee/`), with labrat
//! running as EE. It runs every shared scenario (`scenarios/`, also run against CE by the
//! `conformance` binary) and the EE-only ones (`scenarios_ee/`). Run one with
//! `cargo test -p labrat-conformance --test conformance_ee <name>`.

#[path = "scenarios/mod.rs"]
mod scenarios;
mod scenarios_ee;
