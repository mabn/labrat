# Writes labrat's emoji tables (emoji.txt and emoji-info.tsv) from the tanuki_emoji gem that
# GitLab ships. Run it with `gitlab-rails runner` inside the container; update-data.sh does that.
out = ARGV.fetch(0, "/tmp")
version = Gem.loaded_specs["tanuki_emoji"].version.to_s
major_minor = Gitlab::VERSION.split(".")[0, 2].join(".")
emojis = TanukiEmoji.index.all
File.open(File.join(out, "emoji.txt"), "w") do |f|
  f.puts "# Emoji names and aliases from tanuki_emoji #{version}, as used by GitLab #{major_minor}: name alias..."
  emojis.each { |e| f.puts(([e.name] + e.aliases.map { |a| a.delete(":") }).join(" ")) }
end
File.open(File.join(out, "emoji-info.tsv"), "w") do |f|
  f.puts "# name\tcodepoints\thex\tdescription\tunicode_version (tanuki_emoji #{version})"
  emojis.each { |e| f.puts [e.name, e.codepoints, e.hex, e.description, e.unicode_version].join("\t") }
end
