#!/bin/bash
# Fills a running labrat with demo data to look at in the web UI: users alice and bob
# (password "password123"), the group acme, the project acme/web-app with labels and a
# branch, and a merge request with a diff suggestion, comments and an award emoji.
#
# It RESETS labrat first, deleting everything in its database.
#
#   ./target/release/labrat --db :memory: --root-token glpat-test-token &
#   scripts/seed-demo.sh            # LABRAT_URL and LABRAT_TOKEN override the defaults
set -eo pipefail
L=${LABRAT_URL:-http://localhost:8080}
H="PRIVATE-TOKEN: ${LABRAT_TOKEN:-glpat-test-token}"; J='Content-Type: application/json'; A=$L/api/v4
c() { curl -sf -H "$H" -H "$J" "$@"; }
curl -sf -X POST $L/-/labrat/reset >/dev/null
c -X POST $A/users -d '{"name":"Alice Smith","username":"alice","email":"alice@example.com","password":"password123","skip_confirmation":true}' >/dev/null
c -X POST $A/users -d '{"name":"Bob Jones","username":"bob","email":"bob@example.com","password":"password123","skip_confirmation":true}' >/dev/null
ALICE_T=$(c -X POST $A/users/2/personal_access_tokens -d '{"name":"t","scopes":["api"]}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
G=$(c -X POST $A/groups -d '{"name":"Acme","path":"acme","visibility":"internal"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])')
c -X POST $A/projects -d '{"name":"Web App","path":"web-app","namespace_id":'$G',"initialize_with_readme":true,"description":"The Acme web application"}' >/dev/null
P=$A/projects/acme%2Fweb-app
c -X POST $P/members -d '{"user_id":2,"access_level":30}' >/dev/null
c -X POST $P/members -d '{"user_id":3,"access_level":40}' >/dev/null
c -X POST $P/labels -d '{"name":"bug","color":"#dc143c"}' >/dev/null
c -X POST $P/labels -d '{"name":"type::feature","color":"#6699cc"}' >/dev/null
c -X POST $P/repository/commits -d '{"branch":"main","commit_message":"Add app","actions":[{"action":"create","file_path":"src/app.py","content":"import sys\n\n\ndef main():\n    print(\"hello\")\n    return 0\n\n\nif __name__ == \"__main__\":\n    sys.exit(main())\n"}]}' >/dev/null
curl -sf -H "PRIVATE-TOKEN: $ALICE_T" -H "$J" -X POST $P/repository/commits -d '{"branch":"feature/greeting","start_branch":"main","commit_message":"Greet by name\n\nAdds a name argument.","actions":[{"action":"update","file_path":"src/app.py","content":"import sys\n\n\ndef main(name=\"world\"):\n    print(f\"hello {name}\")\n    return 0\n\n\nif __name__ == \"__main__\":\n    sys.exit(main(*sys.argv[1:]))\n"},{"action":"create","file_path":"docs/usage.md","content":"# Usage\n\nRun `python src/app.py NAME`.\n"}]}' >/dev/null
curl -sf -H "PRIVATE-TOKEN: $ALICE_T" -H "$J" -X POST $P/merge_requests -d '{"source_branch":"feature/greeting","target_branch":"main","title":"Greet users by name","description":"This lets the app greet a user by name.\n\n- [x] Add argument\n- [ ] Update docs\n\nCloses nothing. cc @bob","reviewer_ids":[3,1],"labels":"type::feature"}' >/dev/null
curl -sf $L/-/labrat/idle >/dev/null
MR=$P/merge_requests/1
V=$(c $MR/versions | python3 -c 'import json,sys;v=json.load(sys.stdin)[0];print(v["base_commit_sha"],v["start_commit_sha"],v["head_commit_sha"])')
read B S HD <<< "$V"
c -X POST $MR/discussions -d "{\"body\":\"Should the default be configurable?\n\n\`\`\`suggestion:-0+0\ndef main(name=\\\"there\\\"):\n\`\`\`\",\"position\":{\"base_sha\":\"$B\",\"start_sha\":\"$S\",\"head_sha\":\"$HD\",\"position_type\":\"text\",\"old_path\":\"src/app.py\",\"new_path\":\"src/app.py\",\"new_line\":4}}" >/dev/null
c -X POST $MR/notes -d '{"body":"Looks good overall :+1:"}' >/dev/null
c -X POST $MR/discussions -d '{"body":"Can we add a test for this?"}' >/dev/null
curl -sf -H "PRIVATE-TOKEN: $ALICE_T" -X POST $MR/award_emoji -d 'name=rocket' >/dev/null
echo seeded
