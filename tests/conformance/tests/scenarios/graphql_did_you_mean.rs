//! graphql-ruby's "Did you mean" suggestions in validation errors (`DidYouMean::SpellChecker`):
//! misspelt fields, arguments, input object keys, directives, variable types and fragment types.
//!
//! Unknown names with large dictionaries (fragment types over all types, long names on Mutation)
//! can push GitLab's validation past its 0.2 s budget, which answers "Timeout on validation of
//! query" depending on the moment (docs/gitlab-ref/graphql-validation-timeout.md); the probes
//! here stay clear of that.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_did_you_mean() {
    let mut s = Scenario::new("graphql_did_you_mean").await;
    let admin = s.admin();
    let gpath = s.name("dy");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id()}), "project").await;
    let full = format!("{gpath}/app");
    let probes: Vec<(&str, String)> = vec![
        ("query field typo", "{ currentUsr { id } }".into()),
        ("user field typo", "{ currentUser { usernam } }".into()),
        ("several candidates", "{ currentUser { nam } }".into()),
        ("no candidate", "{ currentUser { zzzz } }".into()),
        ("project field typo", format!("{{ project(fullPath: \"{full}\") {{ mergeRequest(iid: \"1\") {{ titel }} }} }}")),
        ("argument typo", "{ project(fullPth: \"x\") { id } }".into()),
        ("input key typo", "mutation { mergeRequestSetDraft(input: {projectPath: \"x\", iid: \"1\", draf: true}) { errors } }".into()),
        ("directive typo", "{ currentUser { id @skp(if: true) } }".into()),
        ("directive argument typo", "{ currentUser { id @skip(iff: true) } }".into()),
        ("short mutation typo", "mutation { mergeRequestSetDraf(input: {}) { errors } }".into()),
    ];
    for (label, q) in &probes {
        s.graphql(label, Some(&admin), q, json!(null)).await;
    }
    s.finish();
}
