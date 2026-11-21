import { Marked, type TokenizerAndRendererExtension } from 'marked';
import DOMPurify from 'dompurify';

const escape = (s: string) =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

function references(projectPath?: string): TokenizerAndRendererExtension[] {
  return [
    {
      name: 'mrRef',
      level: 'inline',
      start: (src) => src.match(/(?:^|[\s(])!\d/)?.index,
      tokenizer(src) {
        const m = /^!(\d+)/.exec(src);
        if (m && projectPath) return { type: 'mrRef', raw: m[0], iid: m[1] };
        return undefined;
      },
      renderer: (t) => `<a class="gfm-ref" href="/${projectPath}/-/merge_requests/${t.iid}">!${t.iid}</a>`,
    },
    {
      name: 'userRef',
      level: 'inline',
      start: (src) => src.match(/(?:^|[\s(])@[\w.-]/)?.index,
      tokenizer(src) {
        const m = /^@([A-Za-z0-9_][A-Za-z0-9_.-]*[A-Za-z0-9_])/.exec(src);
        if (m) return { type: 'userRef', raw: m[0], username: m[1] };
        return undefined;
      },
      renderer: (t) => `<a class="gfm-user" href="/${escape(t.username)}">@${escape(t.username)}</a>`,
    },
  ];
}

const cache = new Map<string, Marked>();

function instance(projectPath?: string): Marked {
  const key = projectPath ?? '';
  let m = cache.get(key);
  if (!m) {
    m = new Marked({ gfm: true, breaks: false });
    m.use({ extensions: references(projectPath) });
    m.use({
      renderer: {
        code({ text, lang }) {
          if (lang?.startsWith('suggestion')) {
            return `<div class="md-suggestion"><div class="md-suggestion-head">Suggested change</div><pre><code>${escape(text)}</code></pre></div>`;
          }
          return `<pre class="code"><code>${escape(text)}</code></pre>`;
        },
        listitem(item) {
          if (item.task) {
            const box = `<input type="checkbox" disabled${item.checked ? ' checked' : ''}> `;
            return `<li class="task-list-item">${box}${this.parser.parse(item.tokens.filter((t) => t.type !== 'checkbox'))}</li>`;
          }
          return false;
        },
      },
    });
    cache.set(key, m);
  }
  return m;
}

/** Renders GitLab-flavoured Markdown (close enough for reading) to sanitized HTML. */
export function renderMarkdown(src: string, projectPath?: string): string {
  const html = instance(projectPath).parse(src ?? '', { async: false }) as string;
  return DOMPurify.sanitize(html, { ADD_ATTR: ['target'] });
}

/** Removes ```suggestion blocks (they are shown separately with an Apply button). */
export function stripSuggestions(src: string): string {
  return src.replace(/```suggestion[^\n]*\n[\s\S]*?```\n?/g, '').trim();
}
