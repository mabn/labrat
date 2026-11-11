#!/usr/bin/env bash
# Copies the data labrat takes from a specific GitLab version out of the running instance.
#
#   update-data.sh <source dir>
#
# It writes:
#   - crates/labrat-api/data/graphql-introspection.json.gz   the GraphQL schema
#   - crates/labrat-core/src/data/emoji.txt, emoji-info.tsv    the emoji tables
#   - <source dir>/{app,lib,config,db,Gemfile.lock}             the Rails source, for reading
#                                                              (not part of the repository)
# and prints what has to be updated by hand: the version constants, the CI favicon digests
# and the default README template.
#
# Settings (environment variables):
#   GITLAB_CONTAINER   container name (default: gitlab-ce)
set -euo pipefail

SRC="${1:?usage: update-data.sh <source dir>}"
NAME="${GITLAB_CONTAINER:-gitlab-ce}"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
RAILS=/opt/gitlab/embedded/service/gitlab-rails
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

runner() { docker exec "$NAME" gitlab-rails runner "$@" >/dev/null 2>&1; }

echo "Rails source -> $SRC"
mkdir -p "$SRC"
for d in app lib config db Gemfile.lock; do
  rm -rf "${SRC:?}/$d"
  docker cp "$NAME:$RAILS/$d" "$SRC/$d"
done

echo "GraphQL schema"
docker cp "$HERE/dump-graphql-schema.rb" "$NAME:/tmp/dump-graphql-schema.rb"
runner /tmp/dump-graphql-schema.rb /tmp/graphql-introspection.json
docker cp "$NAME:/tmp/graphql-introspection.json" "$TMP/schema.json"
jq -e '.data.__schema.types | length > 0' "$TMP/schema.json" >/dev/null
gzip -9 -n -c "$TMP/schema.json" > "$REPO/crates/labrat-api/data/graphql-introspection.json.gz"

echo "Emoji tables"
docker cp "$HERE/dump-emoji.rb" "$NAME:/tmp/dump-emoji.rb"
runner /tmp/dump-emoji.rb /tmp
docker cp "$NAME:/tmp/emoji.txt" "$REPO/crates/labrat-core/src/data/emoji.txt"
docker cp "$NAME:/tmp/emoji-info.tsv" "$REPO/crates/labrat-core/src/data/emoji-info.tsv"

echo
echo "Update by hand:"
echo "1. crates/labrat-core/src/lib.rs, GITLAB_VERSION and GITLAB_REVISION:"
for f in VERSION REVISION; do echo "     $f: $(docker exec "$NAME" cat "$RAILS/$f")"; done
echo "2. crates/labrat-api/src/routes/pipelines.rs, favicon_digest (the fallback is status_created):"
docker exec "$NAME" ls "$RAILS/public/assets/ci_favicons" | sed 's/^/     /'
echo "3. crates/labrat-api/src/templates/readme_default.md: compare with"
echo "     $SRC/app/views/projects/readme_templates/default.md.tt"
