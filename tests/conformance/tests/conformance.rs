//! All conformance scenarios in one test binary: a binary per scenario would link labrat once per
//! file, which costs gigabytes of build output. Run one with
//! `cargo test -p labrat-conformance --test conformance <name>`.
//!
//! `scenarios/` are shared with the EE binary; `scenarios_ce/` are checked against CE only,
//! until they are recorded against EE too.

mod scenarios;
mod scenarios_ce;
