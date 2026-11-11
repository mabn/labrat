//! Diff fidelity: tricky changes through the commit, compare and MR diff endpoints.

use labrat_conformance::Scenario;
use reqwest::Method;
use serde_json::{Value, json};

const GET: Method = Method::GET;
const POST: Method = Method::POST;

fn numbered(prefix: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{prefix} {i}\n")).collect()
}

#[tokio::test]
async fn diff_fidelity() {
    let mut s = Scenario::new("diff_fidelity").await;
    let admin = s.admin();
    let gpath = s.name("diff");
    let g = s.step("group", POST, "/groups", Some(&admin), json!({"name": gpath, "path": gpath}), "namespace").await;
    let p = s.step("project", POST, "/projects", Some(&admin), json!({"name": "app", "namespace_id": g.id()}), "project").await;
    let pid = p.id();
    let commits = format!("/projects/{pid}/repository/commits");

    // Base content on main.
    let long: String = (1..=300).map(|i| format!("fn item_{i}() {{ println!(\"{i}\"); }}\n")).collect();
    let moved_base: String = numbered("alpha", 10) + &numbered("beta", 10) + &numbered("gamma", 10);
    let big: String = (1..=4000).map(|i| format!("line {i} of a very large generated file with some padding text\n")).collect();
    let medium: String = (1..=1500).map(|i| format!("medium line {i} with a little padding\n")).collect();
    let base_actions = json!([
        {"action": "create", "file_path": "no_newline.txt", "content": "one\ntwo\nthree"},
        {"action": "create", "file_path": "crlf.txt", "content": "a\r\nb\r\nc\r\n"},
        {"action": "create", "file_path": "spaces.txt", "content": "keep\nindent me\ntrail\n"},
        {"action": "create", "file_path": "rename_small.txt", "content": numbered("stable line", 20)},
        {"action": "create", "file_path": "rename_big.txt", "content": numbered("old text", 6)},
        {"action": "create", "file_path": "empty_later.txt", "content": "will be emptied\n"},
        {"action": "create", "file_path": "script.sh", "content": "echo hi\n"},
        {"action": "create", "file_path": "unicode.txt", "content": "zażółć gęślą jaźń\n日本語\n"},
        {"action": "create", "file_path": "long.rs", "content": long},
        {"action": "create", "file_path": "moved.txt", "content": moved_base},
        {"action": "create", "file_path": "big.txt", "content": big},
        {"action": "create", "file_path": "medium.txt", "content": medium},
        {"action": "create", "file_path": "image.bin", "encoding": "base64", "content": "AAECAwQFBgcICQ=="},
        {"action": "create", "file_path": "deleted.txt", "content": "gone soon\n"},
    ]);
    s.step("base", POST, &commits, Some(&admin), json!({"branch": "main", "commit_message": "Base", "actions": base_actions}), "commit").await;

    // The tricky changes, on a branch.
    let mut long_changed: Vec<String> = (1..=300).map(|i| format!("fn item_{i}() {{ println!(\"{i}\"); }}")).collect();
    for i in [5usize, 80, 81, 150, 299] {
        long_changed[i - 1] = format!("fn item_{i}() {{ println!(\"changed {i}\"); }}");
    }
    long_changed.insert(200, "// inserted".to_string());
    let long_changed = long_changed.join("\n") + "\n";
    let moved_new: String = numbered("gamma", 10) + &numbered("alpha", 10) + &numbered("beta", 10);
    let medium_changed: String = (1..=1500).map(|i| if i % 2 == 0 { format!("medium changed {i}\n") } else { format!("medium line {i} with a little padding\n") }).collect();
    let big_changed: String = (1..=4000).map(|i| if i % 3 == 0 { format!("changed {i}\n") } else { format!("line {i} of a very large generated file with some padding text\n") }).collect();
    let mut rename_small = numbered("stable line", 20);
    rename_small = rename_small.replace("stable line 7\n", "edited line 7\n");
    let change_actions = json!([
        {"action": "update", "file_path": "no_newline.txt", "content": "one\ntwo\nthree\nfour"},
        {"action": "update", "file_path": "crlf.txt", "content": "a\r\nB\r\nc\r\n"},
        {"action": "update", "file_path": "spaces.txt", "content": "keep\n    indent me\ntrail   \n"},
        {"action": "move", "previous_path": "rename_small.txt", "file_path": "renamed_small.txt", "content": rename_small},
        {"action": "move", "previous_path": "rename_big.txt", "file_path": "renamed_big.txt", "content": numbered("brand new", 6)},
        {"action": "update", "file_path": "empty_later.txt", "content": ""},
        {"action": "chmod", "file_path": "script.sh", "execute_filemode": true},
        {"action": "update", "file_path": "unicode.txt", "content": "zażółć gęślą jaźń!\n日本語\n한국어\n"},
        {"action": "update", "file_path": "long.rs", "content": long_changed},
        {"action": "update", "file_path": "moved.txt", "content": moved_new},
        {"action": "update", "file_path": "big.txt", "content": big_changed},
        {"action": "update", "file_path": "medium.txt", "content": medium_changed},
        {"action": "update", "file_path": "image.bin", "encoding": "base64", "content": "AAECAwQFBgcICgsM"},
        {"action": "delete", "file_path": "deleted.txt"},
        {"action": "create", "file_path": "new_empty.txt", "content": ""},
        {"action": "create", "file_path": "dir/nested/new.md", "content": "# Title\n\nBody\n"},
    ]);
    let c = s
        .step("changes", POST, &commits, Some(&admin), json!({"branch": "feature", "start_branch": "main", "commit_message": "Tricky changes", "actions": change_actions}), "commit")
        .await;
    let sha = c.get("id").as_str().unwrap().to_string();
    s.settle().await;

    s.step("commit diff", GET, &format!("{commits}/{sha}/diff"), Some(&admin), json!({"per_page": 100}), "diff").await;
    s.step("commit diff unidiff", GET, &format!("{commits}/{sha}/diff"), Some(&admin), json!({"per_page": 100, "unidiff": true}), "diff").await;
    s.step("compare", GET, &format!("/projects/{pid}/repository/compare"), Some(&admin), json!({"from": "main", "to": "feature"}), "compare").await;
    s.step("compare straight", GET, &format!("/projects/{pid}/repository/compare"), Some(&admin), json!({"from": "feature", "to": "main", "straight": true}), "compare").await;

    let mrs = format!("/projects/{pid}/merge_requests");
    s.step("mr", POST, &mrs, Some(&admin), json!({"source_branch": "feature", "target_branch": "main", "title": "Tricky"}), "mr").await;
    s.poll_mr(pid, 1).await;
    s.step("mr diffs", GET, &format!("{mrs}/1/diffs"), Some(&admin), json!({"per_page": 100}), "diff").await;
    s.step("mr diffs page 2", GET, &format!("{mrs}/1/diffs"), Some(&admin), json!({"per_page": 5, "page": 2}), "diff").await;
    s.step("mr changes", GET, &format!("{mrs}/1/changes"), Some(&admin), json!(null), "mr").await;
    s.step("mr raw diffs", GET, &format!("{mrs}/1/raw_diffs"), Some(&admin), json!(null), "diff").await;
    let v = s.step("versions", GET, &format!("{mrs}/1/versions"), Some(&admin), json!(null), "version").await;
    let vid = v.body[0]["id"].as_i64().unwrap_or(0);
    s.step("version", GET, &format!("{mrs}/1/versions/{vid}"), Some(&admin), json!(null), "version").await;
    let _: Value = json!(null);
    s.finish();
}
