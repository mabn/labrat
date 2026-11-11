# Writes GitLab's GraphQL introspection result, including deprecated fields and arguments, as
# JSON. Run it with `gitlab-rails runner` inside the container; update-data.sh does that.
#
# GitLab turns introspection off for its schema outside of requests, so this uses a subclass
# with the entry points enabled.
out = ARGV.fetch(0, "/tmp/graphql-introspection.json")
schema = Class.new(GitlabSchema)
schema.instance_variable_set(:@disable_introspection_entry_points, false)
query = GraphQL::Introspection.query(include_deprecated_args: true)
result = schema.execute(query, context: { introspection: true })
# `to_json` (not `JSON.generate`) escapes like the rest of GitLab, e.g. `>` as `\u003e`.
File.write(out, result.to_h.to_json)
