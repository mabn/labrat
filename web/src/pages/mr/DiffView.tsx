import { Fragment, useMemo, useState, type ReactNode } from 'react';
import type { FileDiff } from '../../api/types';
import { diffStats, lineKey, parseDiff, type DiffLine } from '../../lib/diff';
import { Icon } from '../../components/Icon';

export function fileStatus(f: FileDiff): string | null {
  if (f.new_file) return 'added';
  if (f.deleted_file) return 'deleted';
  if (f.renamed_file) return 'renamed';
  return null;
}

export function fileAnchor(f: FileDiff) {
  return `diff-${(f.new_path || f.old_path).replace(/[^a-zA-Z0-9]/g, '-')}`;
}

export interface LineCommentsProps {
  /** Rendered under the line with this key (`old_new`), if any. */
  renderBelow?: (line: DiffLine) => ReactNode;
  /** Shown as a "+" button on hover; opens a comment form for the line. */
  onAddComment?: (line: DiffLine) => void;
}

/** One file of a diff, in GitLab's inline layout. */
export function DiffFile({ file, comments, headerExtra }: { file: FileDiff; comments?: LineCommentsProps; headerExtra?: ReactNode }) {
  const [collapsed, setCollapsed] = useState(!!file.collapsed || !!file.too_large);
  const lines = useMemo(() => parseDiff(file.diff), [file.diff]);
  const stats = useMemo(() => diffStats(file.diff), [file.diff]);
  const status = fileStatus(file);
  return (
    <div className="diff-file" id={fileAnchor(file)}>
      <div className="diff-file-head">
        <button className="btn btn-tertiary btn-sm btn-icon" onClick={() => setCollapsed(!collapsed)} title={collapsed ? 'Expand file' : 'Collapse file'}>
          <Icon name={collapsed ? 'chevronRight' : 'chevronDown'} />
        </button>
        <span className="path" title={file.new_path}>
          {file.renamed_file ? `${file.old_path} → ${file.new_path}` : file.new_path}
        </span>
        {status && <span className={`badge ${status === 'added' ? 'badge-success' : status === 'deleted' ? 'badge-danger' : 'badge-neutral'}`}>{status}</span>}
        {file.a_mode !== file.b_mode && !file.new_file && !file.deleted_file && (
          <span className="small subtle">
            mode {file.a_mode} → {file.b_mode}
          </span>
        )}
        <span className="added-count">+{stats.added}</span>
        <span className="removed-count">−{stats.removed}</span>
        <span className="spacer" />
        {headerExtra}
      </div>
      {!collapsed &&
        (file.too_large ? (
          <div className="empty">This diff is too large to display.</div>
        ) : !file.diff ? (
          <div className="empty small">{file.renamed_file ? 'File moved' : 'No changes to show (binary file or mode change).'}</div>
        ) : (
          <table className="diff">
            <colgroup>
              <col style={{ width: 50 }} />
              <col style={{ width: 50 }} />
              <col />
            </colgroup>
            <tbody>
              {lines.map((l, i) => {
                if (l.type === 'hunk' || l.type === 'nonewline') {
                  return (
                    <tr key={i} className="hunk">
                      <td className="num">{l.type === 'hunk' ? '…' : ''}</td>
                      <td className="num">{l.type === 'hunk' ? '…' : ''}</td>
                      <td className="code">{l.text}</td>
                    </tr>
                  );
                }
                const below = comments?.renderBelow?.(l);
                return (
                  <Fragment key={i}>
                    <tr className={`line ${l.type}`} data-line={lineKey(l.oldLine, l.newLine)}>
                      <td className="num">
                        {comments?.onAddComment && (
                          <button className="add-comment-btn" title="Add a comment to this line" onClick={() => comments.onAddComment!(l)}>
                            <Icon name="comment" size={12} />
                          </button>
                        )}
                        {l.oldLine ?? ''}
                      </td>
                      <td className="num">{l.newLine ?? ''}</td>
                      <td className="code">{l.text || ' '}</td>
                    </tr>
                    {below && (
                      <tr className="comments">
                        <td colSpan={3}>{below}</td>
                      </tr>
                    )}
                  </Fragment>
                );
              })}
            </tbody>
          </table>
        ))}
    </div>
  );
}

export function FileTree({ files }: { files: FileDiff[] }) {
  return (
    <nav className="file-tree">
      {files.map((f) => {
        const s = diffStats(f.diff);
        const status = fileStatus(f);
        return (
          <a
            key={f.new_path + f.old_path}
            href={`#${fileAnchor(f)}`}
            title={f.new_path}
            onClick={(e) => {
              e.preventDefault();
              document.getElementById(fileAnchor(f))?.scrollIntoView({ behavior: 'smooth', block: 'start' });
            }}
          >
            <Icon name="file" size={14} />
            <span className="truncate" style={{ textDecoration: status === 'deleted' ? 'line-through' : undefined }}>
              {f.new_path.split('/').pop()}
            </span>
            <span className="stat">
              <span className="added-count">+{s.added}</span> <span className="removed-count">−{s.removed}</span>
            </span>
          </a>
        );
      })}
    </nav>
  );
}
