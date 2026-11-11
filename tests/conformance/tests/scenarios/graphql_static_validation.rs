//! graphql-ruby static validation rules not covered elsewhere: undeclared variables, field
//! conflicts under one response key, unused and impossible fragments, fragment cycles, unknown
//! and misplaced directives, duplicate operation, fragment, argument and variable names, input
//! object literals, variable types and default values, and operation selection.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::json;

const POST: Method = Method::POST;

#[tokio::test]
async fn graphql_static_validation() {
    let mut s = Scenario::new("graphql_static_validation").await;
    let admin = s.admin();
    let gpath = s.name("gv");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id(), "initialize_with_readme": true}), "project").await;
    let full = format!("{gpath}/app");
    s.settle().await;
    let v = json!({"p": full});
    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        ("undeclared variable", "{ project(fullPath: $p) { name } }", v.clone()),
        ("undeclared in named", "query Q { project(fullPath: $p) { name } }", v.clone()),
        ("field conflict", "query($p: ID!) { project(fullPath: $p) { x: name x: path } }", v.clone()),
        ("argument conflict", "query($p: ID!) { project(fullPath: $p) { mergeRequests(first: 1) { count } mergeRequests(first: 2) { count } } }", v.clone()),
        ("unused fragment", "query($p: ID!) { project(fullPath: $p) { name } } fragment F on Project { path }", v.clone()),
        ("impossible spread", "query($p: ID!) { project(fullPath: $p) { ...F } } fragment F on User { username }", v.clone()),
        ("fragment cycle", "query($p: ID!) { project(fullPath: $p) { ...A } } fragment A on Project { ...B } fragment B on Project { ...A }", v.clone()),
        ("fragment on scalar", "query($p: ID!) { project(fullPath: $p) { name { ...F } } } fragment F on String { x }", v.clone()),
        ("unknown directive", "query($p: ID!) { project(fullPath: $p) { name @nope } }", v.clone()),
        ("misplaced directive", "query($p: ID!) @skip(if: true) { project(fullPath: $p) { name } }", v.clone()),
        ("duplicate directive", "query($p: ID!) { project(fullPath: $p) { name @skip(if: false) @skip(if: false) } }", v.clone()),
        ("directive missing arg", "query($p: ID!) { project(fullPath: $p) { name @include } }", v.clone()),
        ("duplicate operation", "query A { currentUser { username } } query A { currentUser { name } }", json!(null)),
        ("duplicate fragment", "query($p: ID!) { project(fullPath: $p) { ...F } } fragment F on Project { name } fragment F on Project { path }", v.clone()),
        ("duplicate argument", "query($p: ID!) { project(fullPath: $p, fullPath: $p) { name } }", v.clone()),
        ("duplicate variable", "query($p: ID!, $p: ID!) { project(fullPath: $p) { name } }", v.clone()),
        ("output type variable", "query($p: Project) { currentUser { username } }", json!(null)),
        ("bad default", "query($p: ID! = 5) { project(fullPath: $p) { name } }", json!(null)),
        ("unknown input field literal", "mutation { mergeRequestSetDraft(input: {projectPath: \"x\", iid: \"1\", draft: true, bogus: 1}) { errors } }", json!(null)),
        ("anonymous with others", "{ currentUser { username } } query B { currentUser { name } }", json!(null)),
        ("subscription unknown", "subscription { nope }", json!(null)),
        ("two operations no name", "query A { currentUser { username } } query B { currentUser { name } }", json!(null)),
    ];
    for (label, q, vars) in cases {
        s.graphql(label, Some(&admin), q, vars).await;
    }
    // Undefined fields at the root and below, with and without variables.
    for (label, q, vars) in [
        ("undefined mutation field", "mutation { nopeMutation(input: {a: 1}) { errors } }", json!(null)),
        ("undefined mutation field no args", "mutation { nopeMutation { errors } }", json!(null)),
        ("undefined mutation with vars", "mutation($p: ID!) { nopeMutation(input: {projectPath: $p}) { errors } }", v.clone()),
        ("undefined query field", "{ nopeQuery { id } }", json!(null)),
        ("undefined query field with vars", "query($p: ID!) { nopeQuery(fullPath: $p) { id } }", v.clone()),
    ] {
        s.graphql(label, Some(&admin), q, vars).await;
    }
    s.graphql_named("unknown operation name", Some(&admin), "query A { currentUser { username } }", "B").await;
    // `MAX_QUERY_SIZE`: 10,000 characters, padding included.
    let base = "{ currentUser { username } }";
    let at_limit = format!("{base}{}", " ".repeat(10_000 - base.len()));
    let over = format!("{at_limit} ");
    s.graphql("query at size limit", Some(&admin), &at_limit, json!(null)).await;
    s.graphql("query too large", Some(&admin), &over, json!(null)).await;
    s.step("multiplex too large", POST, "/api/graphql", Some(&admin), json!([{"query": at_limit}, {"query": base}]), "graphql").await;
    s.finish();
}
