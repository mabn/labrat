import { useRef, useState, type ReactNode } from 'react';
import { useMutation } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Discussion, MergeRequest, Note, Project, Suggestion } from '../../api/types';
import { Dropdown, Markdown, TimeAgo, UserAvatar, useToast, errorText } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { emoji, PICKER } from '../../lib/emoji';
import { stripSuggestions } from '../../lib/markdown';
import { useSession } from '../../session';
import { mrBase, useMrRefresh } from './data';

// ---------- Comment form ----------

export function CommentForm({
  projectPath,
  placeholder = 'Write a comment or drag your files here…',
  initial = '',
  autoFocus,
  onCancel,
  actions,
  insertSuggestion,
}: {
  projectPath: string;
  placeholder?: string;
  initial?: string;
  autoFocus?: boolean;
  onCancel?: () => void;
  /** The buttons: each gets the text and a reset callback. The first is also bound to Ctrl+Enter. */
  actions: { label: string; kind?: 'confirm' | 'default'; run: (body: string) => Promise<unknown> }[];
  /** Text for the "Insert suggestion" button, in diff comments. */
  insertSuggestion?: string;
}) {
  const [body, setBody] = useState(initial);
  const [preview, setPreview] = useState(false);
  const [busy, setBusy] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  const toast = useToast();
  const run = async (i: number) => {
    if (!body.trim() || busy) return;
    setBusy(true);
    try {
      await actions[i].run(body);
      setBody('');
      setPreview(false);
    } catch (e) {
      toast(errorText(e), true);
    } finally {
      setBusy(false);
    }
  };
  const insert = (text: string) => {
    const el = ref.current;
    const start = el?.selectionStart ?? body.length;
    const end = el?.selectionEnd ?? body.length;
    const next = body.slice(0, start) + text + body.slice(end);
    setBody(next);
    setPreview(false);
    setTimeout(() => {
      el?.focus();
      el?.setSelectionRange(start + text.length, start + text.length);
    });
  };
  return (
    <div>
      <div className="comment-form">
        <div className="cf-tabs">
          <button type="button" className={!preview ? 'active' : ''} onClick={() => setPreview(false)}>
            Write
          </button>
          <button type="button" className={preview ? 'active' : ''} onClick={() => setPreview(true)}>
            Preview
          </button>
          <span className="spacer" />
          {insertSuggestion !== undefined && (
            <button type="button" title="Insert suggestion" onClick={() => insert(`\`\`\`suggestion:-0+0\n${insertSuggestion}\n\`\`\`\n`)}>
              <Icon name="doc" size={14} /> Suggestion
            </button>
          )}
          <button type="button" title="Add bold text" onClick={() => insert('****')} style={{ fontWeight: 700 }}>
            B
          </button>
          <button type="button" title="Insert code" onClick={() => insert('``')}>
            <Icon name="code" size={14} />
          </button>
        </div>
        {preview ? (
          <div className="preview">{body.trim() ? <Markdown source={body} projectPath={projectPath} /> : <span className="subtle">Nothing to preview.</span>}</div>
        ) : (
          <textarea
            ref={ref}
            value={body}
            autoFocus={autoFocus}
            placeholder={placeholder}
            onChange={(e) => setBody(e.target.value)}
            onKeyDown={(e) => {
              if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
                e.preventDefault();
                run(0);
              } else if (e.key === 'Escape' && onCancel) onCancel();
            }}
          />
        )}
        <div className="cf-hint">Supports Markdown and quick actions (for example /approve, /label ~bug, /assign @user). Ctrl+Enter to submit.</div>
      </div>
      <div className="comment-actions">
        {actions.map((a, i) => (
          <button key={a.label} type="button" className={`btn${a.kind === 'confirm' || (i === 0 && !a.kind) ? ' btn-confirm' : ''}`} disabled={!body.trim() || busy} onClick={() => run(i)}>
            {a.label}
          </button>
        ))}
        {onCancel && (
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </div>
  );
}

// ---------- Award emoji ----------

export function Awards({
  awards,
  onToggle,
  compact,
}: {
  awards: { name: string; username: string }[];
  onToggle: (name: string) => void;
  compact?: boolean;
}) {
  const { user } = useSession();
  const grouped = new Map<string, string[]>();
  for (const a of awards) grouped.set(a.name, [...(grouped.get(a.name) ?? []), a.username]);
  const names = compact ? [...grouped.keys()] : [...new Set(['thumbsup', 'thumbsdown', ...grouped.keys()])];
  if (compact && !names.length) return null;
  return (
    <div className="awards">
      {names.map((n) => {
        const users = grouped.get(n) ?? [];
        const mine = !!user && users.includes(user.username);
        return (
          <button key={n} className={`award${mine ? ' mine' : ''}`} title={users.length ? users.join(', ') : n} onClick={() => user && onToggle(n)}>
            <span>{emoji(n)}</span>
            <span>{users.length}</span>
          </button>
        );
      })}
      {user && <EmojiPicker onPick={onToggle} />}
    </div>
  );
}

export function EmojiPicker({ onPick, small }: { onPick: (name: string) => void; small?: boolean }) {
  return (
    <Dropdown
      button={(_, toggle) => (
        <button type="button" className={small ? 'btn btn-tertiary btn-sm btn-icon' : 'award'} title="Add reaction" onClick={toggle}>
          <Icon name="smile" size={14} />
        </button>
      )}
    >
      {(close) => (
        <div className="emoji-grid">
          {PICKER.map((n) => (
            <button
              key={n}
              title={`:${n}:`}
              onClick={() => {
                close();
                onPick(n);
              }}
            >
              {emoji(n)}
            </button>
          ))}
        </div>
      )}
    </Dropdown>
  );
}

// ---------- Notes and threads ----------

interface ThreadCtx {
  project: Project;
  mr: MergeRequest;
  noteAwards?: Map<number, { name: string; username: string }[]>;
}

function SuggestionBox({ s, ctx, onApplied }: { s: Suggestion; ctx: ThreadCtx; onApplied: () => void }) {
  const toast = useToast();
  const apply = useMutation({
    mutationFn: () => api.put(`/suggestions/${s.id}/apply`, {}),
    onSuccess: () => {
      toast('Suggestion applied');
      onApplied();
    },
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <div className="md-suggestion">
      <div className="md-suggestion-head row">
        Suggested change {s.from_line !== s.to_line ? `(lines ${s.from_line}–${s.to_line})` : ''}
        <span className="spacer" />
        {s.applied ? (
          <span className="badge badge-success">Applied</span>
        ) : (
          ctx.mr.state === 'opened' && (
            <button className="btn btn-sm btn-confirm" disabled={!s.appliable || apply.isPending} title={s.appliable ? 'Apply suggestion' : 'This suggestion can’t be applied'} onClick={() => apply.mutate()}>
              Apply suggestion
            </button>
          )
        )}
      </div>
      <table className="diff">
        <tbody>
          {s.from_content
            .replace(/\n$/, '')
            .split('\n')
            .map((l, i) => (
              <tr key={`d${i}`} className="line del">
                <td className="code">{l || ' '}</td>
              </tr>
            ))}
          {(s.to_content === '' ? [] : s.to_content.replace(/\n$/, '').split('\n')).map((l, i) => (
              <tr key={`a${i}`} className="line add">
                <td className="code">{l || ' '}</td>
              </tr>
            ))}
        </tbody>
      </table>
    </div>
  );
}

export function NoteView({ note, ctx, actions }: { note: Note; ctx: ThreadCtx; actions?: ReactNode }) {
  const { user } = useSession();
  const refresh = useMrRefresh(ctx.project, ctx.mr.iid);
  const toast = useToast();
  const [editing, setEditing] = useState(false);
  const base = mrBase(ctx.project.id, ctx.mr.iid);
  const save = (body: string) => api.put(`${base}/notes/${note.id}`, { body }).then(() => { setEditing(false); refresh(); });
  const del = useMutation({
    mutationFn: () => api.del(`${base}/notes/${note.id}`),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const awards = ctx.noteAwards?.get(note.id) ?? [];
  const toggleAward = async (name: string) => {
    try {
      const mine = awards.find((a) => a.name === name && a.username === user?.username);
      if (mine) {
        const list = await api.get<{ id: number; name: string; user: { username: string } }[]>(`${base}/notes/${note.id}/award_emoji`);
        const a = list.find((x) => x.name === name && x.user.username === user?.username);
        if (a) await api.del(`${base}/notes/${note.id}/award_emoji/${a.id}`);
      } else {
        await api.post(`${base}/notes/${note.id}/award_emoji`, { name });
      }
      refresh();
    } catch (e) {
      toast(errorText(e), true);
    }
  };
  const mine = user?.id === note.author.id;
  const suggestions = note.suggestions ?? [];
  const text = suggestions.length ? stripSuggestions(note.body) : note.body;
  return (
    <div className="note" id={`note_${note.id}`}>
      <div className="note-header">
        <span className="author-name">{note.author.name}</span>
        <span className="subtle small">
          @{note.author.username} · <a href={`#note_${note.id}`}><TimeAgo at={note.created_at} /></a>
          {note.updated_at !== note.created_at && ' · edited'}
        </span>
        {note.internal && <span className="badge badge-warning">Internal note</span>}
        {note.author.id === ctx.mr.author.id && <span className="badge badge-muted">Author</span>}
        <span className="actions">
          {actions}
          {user && <EmojiPicker small onPick={toggleAward} />}
          {mine && (
            <button className="btn btn-tertiary btn-sm btn-icon" title="Edit comment" onClick={() => setEditing(true)}>
              <Icon name="edit" size={14} />
            </button>
          )}
          {(mine || user?.is_admin) && (
            <button className="btn btn-tertiary btn-sm btn-icon" title="Delete comment" onClick={() => confirm('Delete this comment?') && del.mutate()}>
              <Icon name="trash" size={14} />
            </button>
          )}
        </span>
      </div>
      {editing ? (
        <CommentForm projectPath={ctx.project.path_with_namespace} initial={note.body} autoFocus onCancel={() => setEditing(false)} actions={[{ label: 'Save comment', run: save }]} />
      ) : (
        <>
          {text && <Markdown source={text} projectPath={ctx.project.path_with_namespace} />}
          {suggestions.map((s) => (
            <SuggestionBox key={s.id} s={s} ctx={ctx} onApplied={refresh} />
          ))}
          <Awards compact awards={awards} onToggle={toggleAward} />
        </>
      )}
    </div>
  );
}

export function SystemNote({ note, projectPath }: { note: Note; projectPath: string }) {
  const icon = /approved/.test(note.body) ? 'approve' : /merged/.test(note.body) ? 'mr' : /commit/.test(note.body) ? 'commit' : /label/.test(note.body) ? 'label' : /review/.test(note.body) ? 'eye' : /assign/.test(note.body) ? 'users' : /closed/.test(note.body) ? 'close' : /milestone/.test(note.body) ? 'milestone' : /draft|ready/.test(note.body) ? 'edit' : 'history';
  return (
    <div className="system-note" id={`note_${note.id}`}>
      <span className="dot">
        <span>
          <Icon name={icon} size={14} />
        </span>
      </span>
      <div className="grow">
        <strong className="author">{note.author.name}</strong> <span className="subtle">@{note.author.username}</span> <Markdown source={note.body} projectPath={projectPath} /> ·{' '}
        <TimeAgo at={note.created_at} />
      </div>
    </div>
  );
}

/** A discussion thread: the notes, replies and resolve controls. */
export function Thread({ discussion, ctx, header, collapsedByDefault }: { discussion: Discussion; ctx: ThreadCtx; header?: ReactNode; collapsedByDefault?: boolean }) {
  const { user } = useSession();
  const refresh = useMrRefresh(ctx.project, ctx.mr.iid);
  const toast = useToast();
  const [replying, setReplying] = useState(false);
  const [expanded, setExpanded] = useState(!collapsedByDefault || !discussion.resolved);
  const base = mrBase(ctx.project.id, ctx.mr.iid);
  const userNotes = discussion.notes.filter((n) => !n.system);
  const resolve = useMutation({
    mutationFn: (resolved: boolean) => api.put(`${base}/discussions/${discussion.id}`, undefined, { resolved }),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const reply = (body: string) => api.post(`${base}/discussions/${discussion.id}/notes`, { body }).then(() => { setReplying(false); refresh(); });
  const first = userNotes[0] ?? discussion.notes[0];
  const canResolve = discussion.resolvable && !!user;
  const resolvedBy = discussion.notes.find((n) => n.resolved_by)?.resolved_by;
  return (
    <div className="note-card">
      <UserAvatar user={first.author} size={32} />
      <div className="note-body-wrap">
        {header}
        {discussion.resolvable && discussion.resolved && !expanded ? (
          <div className="note row">
            <Icon name="resolve" />
            <span className="discussion-resolved grow">
              {userNotes.length} comment{userNotes.length === 1 ? '' : 's'} · resolved{resolvedBy ? ` by ${resolvedBy.name}` : ''}
            </span>
            <button className="btn btn-sm" onClick={() => setExpanded(true)}>
              Show thread
            </button>
          </div>
        ) : (
          <>
            {discussion.notes.map((n) =>
              n.system ? (
                <div key={n.id} className="note small subtle system-note-inline">
                  <strong>{n.author.name}</strong> <Markdown source={n.body} projectPath={ctx.project.path_with_namespace} />
                </div>
              ) : (
                <NoteView
                  key={n.id}
                  note={n}
                  ctx={ctx}
                  actions={
                    n.id === first.id && canResolve ? (
                      <button className={`btn btn-tertiary btn-sm btn-icon`} title={discussion.resolved ? 'Unresolve thread' : 'Resolve thread'} onClick={() => resolve.mutate(!discussion.resolved)}>
                        <span style={{ color: discussion.resolved ? 'var(--green-text)' : undefined, display: 'inline-flex' }}>
                          <Icon name="resolve" size={14} />
                        </span>
                      </button>
                    ) : undefined
                  }
                />
              ),
            )}
            {!discussion.individual_note && user && (
              <div className="discussion-footer">
                {replying ? (
                  <div className="grow">
                    <CommentForm projectPath={ctx.project.path_with_namespace} placeholder="Reply…" autoFocus onCancel={() => setReplying(false)} actions={[{ label: 'Reply', run: reply }]} />
                  </div>
                ) : (
                  <>
                    <input type="text" className="grow" placeholder="Reply…" readOnly onFocus={() => setReplying(true)} style={{ cursor: 'text' }} />
                    {canResolve && (
                      <button className="btn" onClick={() => resolve.mutate(!discussion.resolved)} disabled={resolve.isPending}>
                        {discussion.resolved ? 'Unresolve thread' : 'Resolve thread'}
                      </button>
                    )}
                  </>
                )}
              </div>
            )}
            {discussion.resolvable && discussion.resolved && collapsedByDefault && (
              <div className="note small">
                <button className="btn btn-sm btn-tertiary" onClick={() => setExpanded(false)}>
                  Hide thread
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
