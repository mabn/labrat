//! Approximate GitLab Flavored Markdown rendering for `*_html` fields.
//! Output is close to GitLab's but not byte-identical (see docs/PLAN.md).

use comrak::{Options, markdown_to_html};

/// `title_html`: the title escaped, without paragraph wrapping.
pub fn render_inline(text: &str) -> String {
    labrat_core::services::notes::html_escape(text)
}

/// The `:plain_markdown` pipeline: CommonMark without GitLab's filters (no `dir="auto"`).
pub fn render_plain(text: &str) -> String {
    let mut opts = Options::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    opts.render.sourcepos = true;
    markdown_to_html(text, &opts).trim_end().to_string()
}

pub fn render(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut opts = Options::default();
    opts.extension.strikethrough = true;
    opts.extension.table = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    opts.render.sourcepos = true;
    let html = markdown_to_html(text, &opts);
    let html = code_blocks(html.trim_end());
    let html = task_lists(&html, text);
    top_level_dir(&html)
}

/// GitLab's code block markup (without Rouge's syntax highlighting tokens): a wrapper div, the
/// canonical language and params, one `span.line` per line, and copy/insert widgets except for
/// suggestions.
fn code_blocks(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<pre data-sourcepos=\"") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(end) = after.find("</code></pre>") else { break };
        let block = &after[..end];
        let pos_start = "<pre data-sourcepos=\"".len();
        let pos = &block[pos_start..pos_start + block[pos_start..].find('"').unwrap_or(0)];
        let code_tag_end = block.find("<code").and_then(|j| block[j..].find('>').map(|k| j + k + 1)).unwrap_or(block.len());
        let code_tag = &block[block.find("<code").unwrap_or(0)..code_tag_end];
        let info = code_tag.split("class=\"language-").nth(1).and_then(|x| x.split('"').next()).unwrap_or("");
        let (lang, params) = match info.split_once(':') {
            Some((l, prm)) => (l.to_string(), Some(prm.to_string())),
            None => (info.to_string(), None),
        };
        let lang = if lang.is_empty() { "plaintext".to_string() } else { lang };
        let content = block[code_tag_end..].strip_suffix('\n').unwrap_or(&block[code_tag_end..]);
        let lines: Vec<String> = content
            .split('\n')
            .enumerate()
            .map(|(n, l)| format!("<span id=\"LC{}\" class=\"line\" lang=\"{lang}\">{l}</span>", n + 1))
            .collect();
        let suggestion = lang == "suggestion";
        out.push_str(&format!(
            "<div class=\"gl-relative markdown-code-block js-markdown-code\"><pre data-sourcepos=\"{pos}\"{canon}{prm} class=\"code highlight js-syntax-highlight language-{lang}\" v-pre=\"true\"><code{cls}>{body}</code></pre>{widgets}</div>",
            canon = if info.is_empty() { String::new() } else { format!(" data-canonical-lang=\"{lang}\"") },
            prm = params.map(|x| format!(" data-lang-params=\"{x}\"")).unwrap_or_default(),
            cls = if suggestion { " class=\"js-render-suggestion\"" } else { "" },
            body = lines.join("\n"),
            widgets = if suggestion { "" } else { "<copy-code></copy-code><insert-code-snippet></insert-code-snippet>" },
        ));
        rest = &after[end + "</code></pre>".len()..];
    }
    out.push_str(rest);
    out
}

/// GitLab's task list markup: `task-list` lists, `task-list-item` items and checkboxes with
/// their source position and an aria label.
fn task_lists(html: &str, source: &str) -> String {
    if !html.contains("<input type=\"checkbox\"") {
        return html.to_string();
    }
    let src_lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<li data-sourcepos=\"") {
        let after = &rest[i..];
        let pos_start = "<li data-sourcepos=\"".len();
        let pos = after[pos_start..].split('"').next().unwrap_or("").to_string();
        let tag_end = after.find('>').map(|k| k + 1).unwrap_or(after.len());
        let body = &after[tag_end..];
        let Some(rest_after_input) = body.strip_prefix("<input type=\"checkbox\" ") else {
            out.push_str(&rest[..i + tag_end]);
            rest = &rest[i + tag_end..];
            continue;
        };
        // Mark the enclosing list.
        let before = &rest[..i];
        match before.rfind("<ul data-sourcepos=\"") {
            Some(u) if !before[u..].contains("</ul>") => {
                out.push_str(&before[..u]);
                out.push_str("<ul class=\"task-list\" ");
                out.push_str(&before[u + 4..]);
            }
            _ => out.push_str(before),
        }
        let checked = rest_after_input.starts_with("checked=\"\" ");
        let input_end = rest_after_input.find("/>").map(|k| k + 2).unwrap_or(0);
        let text_after = &rest_after_input[input_end..];
        let label: String = text_after.split('<').next().unwrap_or("").trim().to_string();
        let line: usize = pos.split(':').next().and_then(|x| x.parse().ok()).unwrap_or(1);
        let col = src_lines.get(line - 1).and_then(|l| l.find('[')).map(|c| c + 2).unwrap_or(1);
        out.push_str(&format!(
            "<li class=\"task-list-item\" data-sourcepos=\"{pos}\"><input type=\"checkbox\" data-checkbox-sourcepos=\"{line}:{col}-{line}:{col}\" class=\"task-list-item-checkbox\"{chk} disabled=\"\" aria-label=\"Check option: {label}\">",
            chk = if checked { " checked=\"\"" } else { "" },
        ));
        rest = text_after;
    }
    out.push_str(rest);
    out
}

/// GitLab adds `dir="auto"` (after `data-sourcepos`) to top-level blocks only.
fn top_level_dir(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut depth = 0usize;
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find('>').map(|k| i + k + 1).unwrap_or(rest.len());
        let tag = &rest[i..end];
        let name: String = tag.trim_start_matches(['<', '/']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let void = matches!(name.as_str(), "input" | "br" | "hr" | "img") || tag.ends_with("/>");
        if tag.starts_with("</") {
            depth = depth.saturating_sub(1);
            out.push_str(tag);
        } else {
            let mut t = tag.to_string();
            if depth == 0 && name != "div" && name != "pre" && t.contains("data-sourcepos=\"") {
                let p = t.find("data-sourcepos=\"").unwrap() + "data-sourcepos=\"".len();
                let q = p + t[p..].find('"').unwrap() + 1;
                t.insert_str(q, " dir=\"auto\"");
            }
            out.push_str(&t);
            if !void {
                depth += 1;
            }
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// GitLab-flavoured Markdown with references: `render` plus the merge request reference filter
/// for `!N` in `p`. GitLab renders when the text is saved and caches the HTML, so only MRs that
/// existed at `saved_at` become links.
pub fn render_gfm(c: &rusqlite::Connection, p: &labrat_core::model::Project, text: &str, saved_at: i64) -> String {
    let html = render(text);
    link_references(c, p, &html, saved_at)
}

fn link_references(c: &rusqlite::Connection, p: &labrat_core::model::Project, html: &str, saved_at: i64) -> String {
    let mut out = String::with_capacity(html.len());
    let mut skip_depth = 0usize; // inside <a>, <code> or <pre>
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('<') {
            let end = stripped.find('>').map(|i| i + 2).unwrap_or(rest.len());
            let tag = &rest[..end];
            let name: String = tag.trim_start_matches(['<', '/']).chars().take_while(|ch| ch.is_ascii_alphanumeric()).collect();
            if matches!(name.as_str(), "a" | "code" | "pre") {
                if tag.starts_with("</") {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if !tag.ends_with("/>") {
                    skip_depth += 1;
                }
            }
            out.push_str(tag);
            rest = &rest[end..];
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        let chunk = &rest[..end];
        if skip_depth > 0 {
            out.push_str(chunk);
        } else {
            let with_users = link_user_refs(c, chunk, UserLinkStyle::Cached);
            out.push_str(&link_text_nodes(&with_users, |t| link_mr_refs(c, p, t, saved_at)));
        }
        rest = &rest[end..];
    }
    out
}

/// `MergeRequestReferenceFilter` for `!N` in a text node.
fn link_mr_refs(c: &rusqlite::Connection, p: &labrat_core::model::Project, text: &str, saved_at: i64) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut last = 0;
    while i < bytes.len() {
        let prev_word = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if bytes[i] == b'!' && !prev_word {
            let digits: String = text[i + 1..].chars().take_while(|ch| ch.is_ascii_digit()).collect();
            let after = i + 1 + digits.len();
            let next_word = after < bytes.len() && (bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_');
            if !digits.is_empty() && !next_word {
                let iid: i64 = digits.parse().unwrap_or(0);
                let found: Option<(i64, String, i64)> = c
                    .query_row(
                        "SELECT id, title, created_at FROM merge_requests WHERE target_project_id = ?1 AND iid = ?2",
                        rusqlite::params![p.id, iid],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .ok();
                // Strictly earlier: an MR's own description is rendered before the MR exists.
                if let Some((id, title, _)) = found.filter(|f| f.2 < saved_at) {
                    out.push_str(&text[last..i]);
                    out.push_str(&format!(
                        "<a href=\"/{path}/-/merge_requests/{iid}\" title=\"{title}\" class=\"gfm gfm-merge_request\" data-original=\"!{iid}\" data-link=\"false\" data-link-reference=\"false\" data-merge-request=\"{id}\" data-project=\"{pid}\" data-project-path=\"{path}\" data-iid=\"{iid}\" data-container=\"body\" data-placement=\"top\" data-reference-type=\"merge_request\">!{iid}</a>",
                        path = p.full_path,
                        title = labrat_core::services::notes::html_escape(&title),
                        pid = p.id,
                    ));
                    i = after;
                    last = after;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

/// How a user reference is rendered: the cached HTML of `body_html` carries the extra
/// `data-original`/`data-container`/`data-placement` attributes; the per-request first line
/// doesn't, and marks the viewer with `current-user`.
#[derive(Clone, Copy)]
pub enum UserLinkStyle<'a> {
    Cached,
    FirstLine(Option<&'a labrat_core::model::User>),
}

/// Applies `f` to the text between tags (used after one filter has already inserted links).
fn link_text_nodes(html: &str, mut f: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_a = 0usize;
    let mut rest = html;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let end = rest.find('>').map(|i| i + 1).unwrap_or(rest.len());
            let tag = &rest[..end];
            if tag.starts_with("<a ") || tag == "<a>" {
                in_a += 1;
            } else if tag == "</a>" {
                in_a = in_a.saturating_sub(1);
            }
            out.push_str(tag);
            rest = &rest[end..];
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        if in_a > 0 {
            out.push_str(&rest[..end]);
        } else {
            out.push_str(&f(&rest[..end]));
        }
        rest = &rest[end..];
    }
    out
}

/// `UserReferenceFilter` for `@username` of existing users.
pub fn link_user_refs(c: &rusqlite::Connection, text: &str, style: UserLinkStyle) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let (mut i, mut last) = (0, 0);
    let is_name = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.';
    while i < bytes.len() {
        let prev_word = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        if bytes[i] == b'@' && !prev_word {
            let mut j = i + 1;
            while j < bytes.len() && is_name(bytes[j]) {
                j += 1;
            }
            // A username doesn't end with a dot.
            while j > i + 1 && bytes[j - 1] == b'.' {
                j -= 1;
            }
            if j > i + 1 {
                let name = &text[i + 1..j];
                if let Ok(Some(u)) = labrat_core::model::User::find_by_username(c, name) {
                    out.push_str(&text[last..i]);
                    let title = labrat_core::services::notes::html_escape(&u.name);
                    match style {
                        UserLinkStyle::Cached => out.push_str(&format!(
                            "<a href=\"/{n}\" title=\"{title}\" class=\"gfm gfm-project_member js-user-link\" data-user=\"{id}\" data-original=\"@{n}\" data-container=\"body\" data-placement=\"top\" data-reference-type=\"user\">@{n}</a>",
                            n = u.username,
                            id = u.id
                        )),
                        UserLinkStyle::FirstLine(viewer) => {
                            let current = if viewer.is_some_and(|v| v.id == u.id) { " current-user" } else { "" };
                            out.push_str(&format!(
                                "<a href=\"/{n}\" title=\"{title}\" class=\"gfm gfm-project_member js-user-link{current}\" data-user=\"{id}\" data-reference-type=\"user\">@{n}</a>",
                                n = u.username,
                                id = u.id
                            ))
                        }
                    }
                    i = j;
                    last = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

/// `first_line_in_markdown` for the common case: the first paragraph's first line rendered
/// inline (no source positions), user references styled per viewer, `...` when more follows.
pub fn first_line_html(c: &rusqlite::Connection, text: &str, viewer: Option<&labrat_core::model::User>) -> String {
    let trimmed = text.trim_start_matches('\n');
    let first = trimmed.lines().next().unwrap_or("");
    // A code block: its first line, in the block's markup without ids or widgets.
    if let Some(info) = first.strip_prefix("```") {
        let (lang, _) = info.split_once(':').unwrap_or((info, ""));
        let lang = if lang.trim().is_empty() { "plaintext" } else { lang.trim() };
        let body: Vec<&str> = trimmed.lines().skip(1).take_while(|l| !l.starts_with("```")).collect();
        let line = body.first().copied().unwrap_or("");
        let more = if body.len() > 1 { "..." } else { "" };
        let cls = if lang == "suggestion" { " class=\"js-render-suggestion\"" } else { "" };
        return format!(
            "<pre class=\"code highlight js-syntax-highlight language-{lang}\"><code{cls}><span class=\"line\" lang=\"{lang}\">{}{more}</span></code></pre>",
            line.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        );
    }
    // A task list: the text of the first item.
    if let Some(item) = first.strip_prefix("- [ ] ").or(first.strip_prefix("- [x] ")).or(first.strip_prefix("* [ ] ")) {
        let more = trimmed.trim_end().len() > first.len();
        return format!("\n {}{}", labrat_core::services::notes::html_escape(item), if more { "..." } else { "" });
    }
    let more = trimmed.trim_end().len() > first.len();
    let mut opts = comrak::Options::default();
    opts.extension.strikethrough = true;
    let html = comrak::markdown_to_html(first, &opts);
    let inner = html.trim().strip_prefix("<p>").and_then(|h| h.strip_suffix("</p>")).unwrap_or(html.trim()).to_string();
    let linked = link_text_nodes(&inner, |t| link_user_refs(c, t, UserLinkStyle::FirstLine(viewer)));
    format!("<p>{linked}{}</p>", if more { "..." } else { "" })
}
