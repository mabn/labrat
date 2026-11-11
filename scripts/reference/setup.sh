#!/usr/bin/env bash
# Prepares a new GitLab instance for recording conformance fixtures, the same way the
# harness prepares labrat (tests/conformance/src/lib.rs, `start_labrat`):
#   - an admin user `claude` (name "Claude"), created by root through the API;
#   - a personal access token `claude-code` for it, with the scopes the scenarios need;
#   - the `rebase_on_merge_automatic` feature flag turned on.
# It writes GITLAB_URL and GITLAB_TOKEN to the env file given as the argument.
#
#   setup.sh <env file>
#
# Settings (environment variables):
#   GITLAB_CONTAINER   container name (default: gitlab-ce)
#
# Needs docker, curl and jq. Run it once, on a fresh instance that has finished starting.
set -euo pipefail

OUT="${1:?usage: setup.sh <env file>}"
NAME="${GITLAB_CONTAINER:-gitlab-ce}"
URL=http://localhost:8929
API="$URL/api/v4"

echo "Waiting for GitLab at $URL..."
until [ "$(curl -s -o /dev/null -w '%{http_code}' "$URL/users/sign_in")" = 200 ]; do sleep 10; done

# A short-lived token for root, made in the Rails console because nothing else exists yet.
ROOT_TOKEN=$(docker exec "$NAME" gitlab-rails runner "
  u = User.find_by_username('root')
  t = u.personal_access_tokens.create!(name: 'labrat-setup', scopes: ['api'], expires_at: 1.day.from_now, organization: u.organization)
  puts 'TOKEN=' + t.token
" 2>/dev/null | sed -n 's/^TOKEN=//p')
[ -n "$ROOT_TOKEN" ] || { echo "could not create a root token" >&2; exit 1; }
revoke_root_token() { curl -s -o /dev/null -X DELETE -H "PRIVATE-TOKEN: $ROOT_TOKEN" "$API/personal_access_tokens/self"; }
trap revoke_root_token EXIT

api() {
  local method=$1 path=$2 body=${3:-}
  curl -sS --fail-with-body -X "$method" -H "PRIVATE-TOKEN: $ROOT_TOKEN" -H 'Content-Type: application/json' \
    ${body:+--data "$body"} "$API$path"
}

if [ "$(api GET '/users?username=claude' | jq length)" != 0 ]; then
  echo "user claude already exists; this instance is set up already" >&2
  exit 1
fi

# The same request the harness sends to labrat.
USER_ID=$(api POST /users '{"username": "claude", "name": "Claude", "email": "claude@gitlab.unhood.org",
  "password": "claude-password-123", "admin": true, "skip_confirmation": true}' | jq -r .id)

EXPIRES=$(date -d '+364 days' +%F)
TOKEN=$(api POST "/users/$USER_ID/personal_access_tokens" "{\"name\": \"claude-code\",
  \"scopes\": [\"api\", \"read_repository\", \"write_repository\", \"admin_mode\"], \"expires_at\": \"$EXPIRES\"}" | jq -r .token)

api POST /features/rebase_on_merge_automatic '{"value": "true"}' >/dev/null

umask 077
cat > "$OUT" <<EOF
GITLAB_URL=$URL
GITLAB_TOKEN=$TOKEN
# User claude (id $USER_ID); token expires $EXPIRES; scopes: api, read_repository, write_repository, admin_mode
EOF
echo "Done. Wrote $OUT. The token expires $EXPIRES."
