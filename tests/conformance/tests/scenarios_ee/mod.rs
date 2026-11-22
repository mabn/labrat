//! Scenarios for EE-only features. They run only in the `conformance_ee` binary.
pub mod approval_settings;
mod approval_rules_project;
mod approval_mr_flow;
mod graphql_approvals;
pub mod merge_train_settings;
mod merge_train_flow;
mod merge_train_drops;
mod graphql_merge_trains;
mod merge_train_hooks;
mod mr_dependencies;
mod code_owners;
mod approval_policies;
mod approval_policies_variants;
mod approval_policy_settings;
