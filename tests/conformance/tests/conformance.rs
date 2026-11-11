//! All conformance scenarios in one test binary: a binary per scenario would link labrat once per
//! file, which costs gigabytes of build output. Run one with
//! `cargo test -p labrat-conformance --test conformance <name>`.

mod scenarios;
