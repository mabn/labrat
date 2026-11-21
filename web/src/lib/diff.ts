// Parses a unified diff (as GitLab's API returns per file) into rows for display.

export type LineType = 'context' | 'add' | 'del' | 'hunk' | 'nonewline';

export interface DiffLine {
  type: LineType;
  oldLine: number | null;
  newLine: number | null;
  text: string;
}

export function parseDiff(diff: string): DiffLine[] {
  const out: DiffLine[] = [];
  let oldNo = 0;
  let newNo = 0;
  const lines = diff.split('\n');
  if (lines.length && lines[lines.length - 1] === '') lines.pop();
  for (const line of lines) {
    if (line.startsWith('@@')) {
      const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      if (m) {
        oldNo = Number(m[1]);
        newNo = Number(m[2]);
      }
      out.push({ type: 'hunk', oldLine: null, newLine: null, text: line });
    } else if (line.startsWith('+')) {
      out.push({ type: 'add', oldLine: null, newLine: newNo++, text: line.slice(1) });
    } else if (line.startsWith('-')) {
      out.push({ type: 'del', oldLine: oldNo++, newLine: null, text: line.slice(1) });
    } else if (line.startsWith('\\')) {
      out.push({ type: 'nonewline', oldLine: null, newLine: null, text: line });
    } else {
      out.push({ type: 'context', oldLine: oldNo++, newLine: newNo++, text: line.slice(1) });
    }
  }
  return out;
}

export function diffStats(diff: string): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const l of diff.split('\n')) {
    if (l.startsWith('+')) added++;
    else if (l.startsWith('-')) removed++;
  }
  return { added, removed };
}

/** A key identifying a diff line within a file, matching how positions name lines. */
export function lineKey(oldLine: number | null, newLine: number | null): string {
  return `${oldLine ?? ''}_${newLine ?? ''}`;
}
