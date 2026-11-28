//! GitLab's query analyzers, run after validation: the recursion check
//! (`Gitlab::Graphql::QueryAnalyzers::AST::RecursionAnalyzer`), the maximum depth and the query
//! complexity (graphql-ruby's legacy complexity with GitLab's `Types::BaseField` costs; see
//! docs/gitlab-ref/graphql-complexity.md). Calibrated against `queryComplexity` scores recorded
//! in the `graphql_complexity` scenario.

use async_graphql_parser::Positioned;
use async_graphql_parser::types::{ExecutableDocument, Field, OperationDefinition, OperationType, Selection, SelectionSet};
use async_graphql_value::Value as GValue;
use serde_json::{Map, Value, json};
use std::cell::Cell;

thread_local! {
    /// The current query's score and limit, for `Query.queryComplexity`.
    pub static SCORE: Cell<(i64, i64)> = const { Cell::new((0, 0)) };
}

/// How a field adds to the complexity.
#[derive(Clone, Copy)]
enum Cost {
    /// `base + child` (1, or 2 for `calls_gitaly`).
    Plain(i64),
    /// An explicit `complexity: n`: `n + child`, nothing else.
    Const(i64),
    /// A resolver field: `child + resolver_complexity (+1 gitaly)`, scaled by the page size when
    /// it returns a connection.
    Resolver(Res),
}

#[derive(Clone, Copy, Default)]
struct Res {
    gitaly: bool,
    /// The `sort` argument has a default, so it always counts.
    sort_default: bool,
    conn: bool,
    max_page: i64,
    extra: Extra,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Extra {
    #[default]
    None,
    /// `BlobsResolver`: `+ paths.size`.
    Paths,
    /// `ResolvesPipelines`: `+2` with `sha`, `+2` with `ref`.
    Pipelines,
    /// `NamespaceProjectsResolver`: `+10`.
    Plus10,
    /// `Projects::SnippetsResolver`: multiplier 0.05 (0 with `iid`/`iids`).
    Snippets,
    /// `Ci::ProjectPipelineResolver`: no resolver complexity without arguments, and `1 - 10`
    /// with any of `id`, `iid` or `sha`.
    ProjectPipeline,
}

fn res(conn: bool, sort_default: bool, gitaly: bool) -> Cost {
    Cost::Resolver(Res { gitaly, sort_default, conn, max_page: 100, extra: Extra::None })
}

fn user_like(t: &str) -> bool {
    matches!(t, "UserCore" | "CurrentUser" | "User" | "MergeRequestAuthor" | "MergeRequestReviewer" | "MergeRequestAssignee" | "MergeRequestParticipant" | "AutocompletedUser")
}

/// Costs that differ from the default `1 + child`.
fn cost(parent: &str, field: &str) -> Cost {
    use Cost::*;
    match (parent, field) {
        ("Query", "ciConfig") => Const(126),
        ("Query", "timelogs") => Const(5),
        ("Query", "users") | ("Query", "projects") | ("Query", "groups") => res(true, true, false),
        ("Project", "mergeRequests") | ("Project", "issues") | ("Project", "milestones") | ("Project", "releases") | ("Project", "workItems")
        | ("Project", "pipelineSchedules") | ("Group", "mergeRequests") | ("Group", "milestones") | ("Group", "issues") => res(true, true, false),
        ("Project", "labels") | ("Project", "projectMembers") | ("Group", "labels") | ("Group", "groupMembers") => res(true, false, false),
        ("Project", "pipelines") | ("Commit", "pipelines") => Resolver(Res { gitaly: true, conn: true, max_page: 100, extra: Extra::Pipelines, ..Default::default() }),
        ("Project", "snippets") => Resolver(Res { conn: true, max_page: 100, extra: Extra::Snippets, ..Default::default() }),
        ("Project", "boards") | ("Group", "boards") => Resolver(Res { conn: true, max_page: 2000, ..Default::default() }),
        ("Project", "branchRules") => res(true, false, true),
        ("Project", "forkDetails") | ("Project", "languages") | ("Project", "protectableBranches") | ("Project", "forksCount") | ("Project", "avatarUrl")
        | ("Project", "statisticsDetailsPaths") | ("Project", "ciConfigVariables") | ("Project", "ciPipelineCreationInputs") | ("Project", "sastCiConfiguration") => Plain(2),
        ("Project", "commitReferences") => res(false, false, true),
        ("Project", "descriptionHtml") | ("Project", "timelogs") | ("Group", "timelogs") | ("Group", "descendantGroups") | ("Namespace", "descriptionHtml")
        | ("Group", "descriptionHtml") | ("Label", "descriptionHtml") | ("Release", "descriptionHtml") => Const(5),
        ("Release", "commit") => Const(10),
        ("Namespace", "projects") | ("Group", "projects") => Resolver(Res { sort_default: true, conn: true, max_page: 100, extra: Extra::Plus10, ..Default::default() }),
        ("MergeRequest", "notes") => res(true, false, false),
        ("MergeRequest", "closedAt") | ("MergeRequest", "mergedAt") | ("MergeRequest", "subscribed") | ("MergeRequest", "assignees") | ("MergeRequest", "reviewers")
        | ("MergeRequest", "labels") | ("MergeRequest", "committers") | ("MergeRequest", "titleHtml") | ("MergeRequest", "descriptionHtml") => Const(5),
        ("MergeRequest", "participants") => Const(15),
        (
            "MergeRequest",
            "diffStats" | "diffStatsSummary" | "diffHeadSha" | "sourceBranchProtected" | "targetBranchPath" | "defaultMergeCommitMessage" | "defaultSquashCommitMessage"
            | "divergedFromTargetBranch" | "detailedMergeStatus" | "mergeabilityChecks" | "mergeableDiscussionsState" | "rebaseInProgress" | "shouldBeRebased"
            | "sourceBranchExists" | "targetBranchExists" | "approved" | "availableAutoMergeStrategies" | "mergeable" | "commits" | "commitsWithoutMergeCommits",
        ) => Plain(2),
        ("MergeRequest", "pipelines") => Resolver(Res { gitaly: true, conn: true, max_page: 500, extra: Extra::Pipelines, ..Default::default() }),
        ("MergeRequest", "linkedWorkItems") => res(false, false, true),
        ("Note", "bodyHtml" | "authorIsContributor") => Plain(2),
        (t, "timelogs") if user_like(t) => Const(5),
        (t, "authoredMergeRequests" | "assignedMergeRequests" | "reviewRequestedMergeRequests" | "assigneeOrReviewerMergeRequests") if user_like(t) => res(true, true, false),
        (t, "todos" | "groups" | "starredProjects" | "contributedProjects" | "snippets") if user_like(t) => res(true, false, false),
        ("UserMergeRequestInteraction", "canMerge") => Plain(2),
        ("Project", "pipeline") => Resolver(Res { gitaly: true, extra: Extra::ProjectPipeline, ..Default::default() }),
        ("Project", "jobs") => res(true, false, false),
        ("Pipeline", "beforeSha" | "commit" | "latest" | "refText") => Plain(2),
        ("Pipeline", "stages" | "jobs") => res(true, false, false),
        ("DetailedStatus", "action" | "label") | ("CiJob" | "CiJobInterface", "canPlayJob") | ("JobPermissions", "updateBuild" | "cancelBuild") => Plain(2),
        ("PipelinePermissions", "updatePipeline" | "cancelPipeline") => Plain(2),
        ("Commit", "title" | "fullTitle" | "signature" | "signatureHtml" | "diffs") => Plain(2),
        ("Commit", "titleHtml" | "fullTitleHtml" | "descriptionHtml") => Const(5),
        ("Repository", "blobs") => Resolver(Res { gitaly: true, conn: true, max_page: 100, extra: Extra::Paths, ..Default::default() }),
        ("Repository", "branchNames") => Const(170),
        ("Repository", "commit" | "lastCommit" | "tree") => res(false, false, true),
        ("Repository", "commits" | "paginatedTree") => res(true, false, true),
        ("Repository", "empty" | "exists" | "rootRef") => Plain(2),
        ("RepositoryBlob", "rawBlob" | "rawTextBlob") => Const(2),
        ("RepositoryBlob", "base64EncodedBlob") => Const(3),
        ("RepositoryBlob", "plainData") => Const(5),
        (
            "RepositoryBlob",
            "lfsOid" | "permalinkPath" | "environmentFormattedExternalUrl" | "environmentExternalUrlForRouteMap" | "canModifyBlob" | "canCurrentUserPushToBranch"
            | "language" | "codeNavigationPath",
        ) => Plain(2),
        ("Blob", "lfsOid") => Plain(2),
        ("Tree", "lastCommit") => Const(10),
        ("Tree", "permalinkPath" | "submodules" | "blobs") => Plain(2),
        ("Todo", "target" | "targetEntity" | "body") => Plain(2),
        ("MergeRequestPermissions", "pushToSourceBranch" | "removeSourceBranch" | "cherryPickOnCurrentMergeRequest" | "revertOnCurrentMergeRequest" | "canMerge" | "canApprove") => {
            Plain(2)
        }
        _ => Plain(1),
    }
}

struct Ctx<'a> {
    doc: &'a ExecutableDocument,
    vars: &'a Map<String, Value>,
}

impl Ctx<'_> {
    fn value(&self, v: &GValue) -> Value {
        match v {
            GValue::Variable(n) => self.vars.get(n.as_str()).cloned().unwrap_or(Value::Null),
            other => other.clone().into_json().unwrap_or(Value::Null),
        }
    }

    fn arg(&self, f: &Field, name: &str) -> Option<Value> {
        f.arguments.iter().find(|(n, _)| n.node.as_str() == name).map(|(_, v)| self.value(&v.node)).filter(|v| !v.is_null())
    }

    /// `@skip(if: true)` / `@include(if: false)`.
    fn skipped(&self, directives: &[Positioned<async_graphql_parser::types::Directive>]) -> bool {
        directives.iter().any(|d| {
            let cond = d.node.arguments.iter().find(|(n, _)| n.node.as_str() == "if").map(|(_, v)| self.value(&v.node)).and_then(|v| v.as_bool());
            match d.node.name.node.as_str() {
                "skip" => cond == Some(true),
                "include" => cond == Some(false),
                _ => false,
            }
        })
    }

    /// The fields of a selection set (fragments flattened), with the type they are selected on.
    fn fields<'s>(&'s self, set: &'s SelectionSet, parent: &str, out: &mut Vec<(&'s Field, String)>) {
        self.fields_in(set, parent, out, &mut std::collections::HashSet::new());
    }

    /// `CollectFields`: each fragment is collected once per selection set (`visitedFragments`).
    fn fields_in<'s>(&'s self, set: &'s SelectionSet, parent: &str, out: &mut Vec<(&'s Field, String)>, visited: &mut std::collections::HashSet<String>) {
        for sel in &set.items {
            match &sel.node {
                Selection::Field(f) => {
                    if !self.skipped(&f.node.directives) {
                        out.push((&f.node, parent.to_string()));
                    }
                }
                Selection::InlineFragment(i) => {
                    if self.skipped(&i.node.directives) {
                        continue;
                    }
                    let ty = i.node.type_condition.as_ref().map(|t| t.node.on.node.to_string()).unwrap_or_else(|| parent.to_string());
                    self.fields_in(&i.node.selection_set.node, &ty, out, visited);
                }
                Selection::FragmentSpread(s) => {
                    if self.skipped(&s.node.directives) {
                        continue;
                    }
                    if visited.insert(s.node.fragment_name.node.to_string())
                        && let Some(fd) = self.doc.fragments.get(&s.node.fragment_name.node)
                    {
                        let ty = fd.node.type_condition.node.on.node.to_string();
                        self.fields_in(&fd.node.selection_set.node, &ty, out, visited);
                    }
                }
            }
        }
    }

    fn field_type(parent: &str, name: &str) -> String {
        if name == "__typename" {
            return "String".into();
        }
        super::schema::schema().get(parent).and_then(|t| t.field(name)).map(|f| f.ty.named().to_string()).unwrap_or_default()
    }

    fn complexity(&self, set: &SelectionSet, parent: &str) -> i64 {
        let mut fields = vec![];
        self.fields(set, parent, &mut fields);
        // Fields with the same response key (and type) are merged and counted once.
        let mut seen: Vec<(String, String)> = vec![];
        fields.retain(|(f, on)| {
            let key = (f.alias.as_ref().map(|a| a.node.to_string()).unwrap_or_else(|| f.name.node.to_string()), on.clone());
            if seen.contains(&key) {
                false
            } else {
                seen.push(key);
                true
            }
        });
        let mut total = 0;
        for (f, on) in fields {
            let ty = Self::field_type(&on, f.name.node.as_str());
            let child = if f.selection_set.node.items.is_empty() { 0 } else { self.complexity(&f.selection_set.node, &ty) };
            total += self.field_cost(f, &on, child);
        }
        total
    }

    fn field_cost(&self, f: &Field, parent: &str, child: i64) -> i64 {
        match cost(parent, f.name.node.as_str()) {
            Cost::Plain(b) => b + child,
            Cost::Const(n) => n + child,
            Cost::Resolver(r) if r.extra == Extra::ProjectPipeline => {
                let rc = if ["id", "iid", "sha"].iter().any(|a| self.arg(f, a).is_some()) { -9 } else { 0 };
                // A negative cost counts as 0 (recorded: `pipeline(iid:) { id }` adds nothing).
                (child + rc + 1).max(0)
            }
            Cost::Resolver(r) => {
                let mut rc = 1;
                if r.sort_default || self.arg(f, "sort").is_some() {
                    rc += 1;
                }
                if self.arg(f, "search").is_some() {
                    rc += 5;
                }
                match r.extra {
                    Extra::Paths => rc += self.arg(f, "paths").and_then(|v| v.as_array().map(|a| a.len() as i64)).unwrap_or(0),
                    Extra::Pipelines => {
                        rc += if self.arg(f, "sha").is_some() { 2 } else { 0 } + if self.arg(f, "ref").is_some() { 2 } else { 0 };
                    }
                    Extra::Plus10 => rc += 10,
                    _ => {}
                }
                let mut c = (child + rc + i64::from(r.gitaly)) as f64;
                if r.conn {
                    let limit = [self.arg(f, "first"), self.arg(f, "last")]
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_i64())
                        .chain([r.max_page])
                        .min()
                        .unwrap_or(r.max_page) as f64;
                    let mult = if r.extra == Extra::Snippets {
                        if self.arg(f, "iid").is_some() || self.arg(f, "iids").is_some() { 0.0 } else { 0.05 }
                    } else if let Some(iids) = self.arg(f, "iids").and_then(|v| v.as_array().map(|a| a.len())) {
                        match iids {
                            0 => 0.01,
                            1..=100 => 0.05,
                            101..=1000 => 0.5,
                            _ => 5.0,
                        }
                    } else if self.arg(f, "iid").is_some() {
                        0.0
                    } else {
                        0.01
                    };
                    c += c * (limit * mult);
                }
                c as i64
            }
        }
    }

    fn depth(&self, set: &SelectionSet, parent: &str) -> usize {
        let mut fields = vec![];
        self.fields(set, parent, &mut fields);
        fields
            .iter()
            .map(|(f, on)| {
                let ty = Self::field_type(on, f.name.node.as_str());
                1 + if f.selection_set.node.items.is_empty() { 0 } else { self.depth(&f.selection_set.node, &ty) }
            })
            .max()
            .unwrap_or(0)
    }

    /// `RecursionAnalyzer`: per branch, how many times each field with a selection repeats.
    fn recursion(&self, set: &SelectionSet, parent: &str, stack: &mut Vec<(String, usize)>, found: &mut Vec<(String, usize)>) {
        let mut fields = vec![];
        self.fields(set, parent, &mut fields);
        for (f, on) in fields {
            if f.selection_set.node.items.is_empty() {
                continue;
            }
            let name = f.name.node.to_string();
            let ignored = matches!(name.as_str(), "node" | "edges" | "nodes" | "ofType");
            if !ignored {
                let n = stack.iter().filter(|(s, _)| *s == name).count() + 1;
                stack.push((name.clone(), n));
                if n > 2 {
                    match found.iter_mut().find(|(k, _)| *k == name) {
                        Some(e) => e.1 = n,
                        None => found.push((name.clone(), n)),
                    }
                }
            }
            let ty = Self::field_type(&on, f.name.node.as_str());
            self.recursion(&f.selection_set.node, &ty, stack, found);
            if !ignored {
                stack.pop();
            }
        }
    }
}

/// Runs the analyzers. Returns the errors that stop the query (no data is returned with them).
pub fn analyze(doc: &ExecutableDocument, op: &Positioned<OperationDefinition>, vars: &Map<String, Value>, user: Option<&labrat_core::model::User>) -> Option<Vec<Value>> {
    let cx = Ctx { doc, vars };
    let root = match op.node.ty {
        OperationType::Mutation => super::schema::schema().mutation_type.clone(),
        _ => super::schema::schema().query_type.clone(),
    };
    let set = &op.node.selection_set.node;
    let mut errors = vec![];
    let mut found = vec![];
    cx.recursion(set, &root, &mut vec![], &mut found);
    if !found.is_empty() {
        let hash = found.iter().map(|(k, v)| format!("\"{k}\"=>{v}")).collect::<Vec<_>>().join(", ");
        errors.push(json!({"message": format!("Recursive query - too many of fields '{{{hash}}}' detected\nin single branch of the query\")\n")}));
    }
    let max_depth = if user.is_some() { 20 } else { 15 };
    let depth = cx.depth(set, &root);
    if depth > max_depth {
        errors.push(json!({"message": format!("Query has depth of {depth}, which exceeds max depth of {max_depth}")}));
    }
    let limit = match user {
        Some(u) if u.admin => 300,
        Some(_) => 250,
        None => 200,
    };
    let score = cx.complexity(set, &root);
    SCORE.with(|s| s.set((score, limit)));
    if score > limit {
        errors.push(json!({"message": format!("Query has complexity of {score}, which exceeds max complexity of {limit}")}));
    }
    (!errors.is_empty()).then_some(errors)
}
