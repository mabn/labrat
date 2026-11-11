#!/usr/bin/env python3
"""Runs the same merge request workload against labrat or GitLab and reports latencies.

    workload.py <base url> <admin token> [iterations]

Each iteration creates a project with a README, commits a file to a new branch, opens a merge
request, waits until GitLab reports it mergeable, comments on it, merges it, and reads
it back over REST and GraphQL. Prints one JSON object with the timings in milliseconds.

Uses only the Python standard library.
"""

import json
import statistics
import sys
import time
import urllib.error
import urllib.request

BASE = sys.argv[1].rstrip("/")
TOKEN = sys.argv[2]
ITERATIONS = int(sys.argv[3]) if len(sys.argv) > 3 else 20
RUN = f"bench{int(time.time())}"

latencies: dict[str, list[float]] = {}


def call(op: str, method: str, path: str, body=None, record=True):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        BASE + path,
        data=data,
        method=method,
        headers={"PRIVATE-TOKEN": TOKEN, "Content-Type": "application/json"},
    )
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(req) as r:
            payload = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        payload = e.read()
        status = e.code
    elapsed = (time.perf_counter() - start) * 1000
    if record:
        latencies.setdefault(op, []).append(elapsed)
    if status >= 400:
        raise RuntimeError(f"{op}: {method} {path} -> {status} {payload[:300]!r}")
    return json.loads(payload) if payload else None


def iteration(i: int) -> float:
    start = time.perf_counter()
    p = call("create project", "POST", "/api/v4/projects", {"name": f"{RUN}-{i}", "initialize_with_readme": True})
    pid = p["id"]
    call("commit to a new branch", "POST", f"/api/v4/projects/{pid}/repository/commits", {
        "branch": "feature",
        "start_branch": p["default_branch"],
        "commit_message": "Add feature",
        "actions": [{"action": "create", "file_path": "feature.txt", "content": "hello\n" * 50}],
    })
    mr = call("open merge request", "POST", f"/api/v4/projects/{pid}/merge_requests", {
        "source_branch": "feature",
        "target_branch": p["default_branch"],
        "title": "Add feature",
    })
    iid = mr["iid"]
    mr_path = f"/api/v4/projects/{pid}/merge_requests/{iid}"
    # Mergeability is computed in the background on GitLab.
    wait_start = time.perf_counter()
    while call("get merge request", "GET", mr_path)["detailed_merge_status"] != "mergeable":
        time.sleep(0.1)
    latencies.setdefault("wait until mergeable", []).append((time.perf_counter() - wait_start) * 1000)
    call("comment", "POST", f"{mr_path}/notes", {"body": "Looks good"})
    call("merge", "PUT", f"{mr_path}/merge")
    call("list merged MRs", "GET", f"/api/v4/projects/{pid}/merge_requests?state=merged")
    call("GraphQL query", "POST", "/api/graphql", {
        "query": "query($p: ID!) { project(fullPath: $p) { mergeRequest(iid: \"%s\") { title state mergedAt notes { count } } } }" % iid,
        "variables": {"p": p["path_with_namespace"]},
    })
    return (time.perf_counter() - start) * 1000


def main():
    totals = [iteration(i) for i in range(ITERATIONS)]
    result = {
        "iterations": ITERATIONS,
        "total_s": round(sum(totals) / 1000, 2),
        "iteration_median_ms": round(statistics.median(totals), 1),
        "median_ms": {op: round(statistics.median(v), 1) for op, v in latencies.items()},
    }
    print(json.dumps(result, indent=2))


main()
