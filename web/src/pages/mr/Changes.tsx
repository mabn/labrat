import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { DiffRefs, Discussion, DraftNote, FileDiff, MergeRequest, MrVersion, Position, Project } from '../../api/types';
import { Empty, ErrorBox, Loading, Markdown, useToast, errorText } from '../../components/ui';
import { lineKey, type DiffLine } from '../../lib/diff';
import { useSession } from '../../session';
import { mrBase, useMrRefresh } from './data';
import { DiffFile, FileTree } from './DiffView';
import { CommentForm, Thread } from './Discussions';
import { Icon } from '../../components/Icon';

const posKey = (p: Pick<Position, 'old_line' | 'new_line'>) => lineKey(p.old_line, p.new_line);
const fileKey = (f: FileDiff) => `${f.old_path}\u0000${f.new_path}`;

export function Changes({
  project,
  mr,
  discussions,
  drafts,
  noteAwards,
}: {
  project: Project;
  mr: MergeRequest;
  discussions: Discussion[];
  drafts: DraftNote[];
  noteAwards?: Map<number, { name: string; username: string }[]>;
}) {
  const { user } = useSession();
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const versions = useQuery({ queryKey: ['mr', project.id, mr.iid, 'versions'], queryFn: () => api.get<MrVersion[]>(`${base}/versions`) });
  const [versionId, setVersionId] = useState<number | 'latest'>('latest');
  const latest = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'diffs', mr.sha],
    queryFn: () => api.all<FileDiff>(`${base}/diffs`, { per_page: 100 }, 30),
    enabled: versionId === 'latest',
  });
  const older = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'version', versionId],
    queryFn: () => api.get<MrVersion & { diffs: FileDiff[] }>(`${base}/versions/${versionId}`),
    enabled: versionId !== 'latest',
  });
  const files = versionId === 'latest' ? latest.data : older.data?.diffs;
  const refs: DiffRefs | null | undefined =
    versionId === 'latest'
      ? mr.diff_refs
      : older.data && { base_sha: older.data.base_commit_sha, head_sha: older.data.head_commit_sha, start_sha: older.data.start_commit_sha };
  const [open, setOpen] = useState<{ file: string; line: string } | null>(null);
  const [showComments, setShowComments] = useState(true);

  // Diff threads and pending comments by file and line.
  const byLine = useMemo(() => {
    const m = new Map<string, { threads: Discussion[]; drafts: DraftNote[] }>();
    const slot = (k: string) => {
      let v = m.get(k);
      if (!v) m.set(k, (v = { threads: [], drafts: [] }));
      return v;
    };
    for (const d of discussions) {
      const p = d.notes[0]?.position;
      if (!p || d.notes[0].type !== 'DiffNote') continue;
      if (refs && versionId !== 'latest' && p.head_sha !== refs.head_sha) continue;
      slot(`${p.new_path}|${posKey(p)}`).threads.push(d);
      if (p.old_path !== p.new_path) slot(`${p.old_path}|${posKey(p)}`);
    }
    for (const d of drafts) {
      const p = d.position;
      if (!p?.new_path || d.discussion_id) continue;
      slot(`${p.new_path}|${posKey(p)}`).drafts.push(d);
    }
    return m;
  }, [discussions, drafts, refs, versionId]);

  const position = (f: FileDiff, l: DiffLine) => ({
    base_sha: refs!.base_sha,
    start_sha: refs!.start_sha,
    head_sha: refs!.head_sha,
    position_type: 'text',
    old_path: f.old_path,
    new_path: f.new_path,
    old_line: l.type === 'add' ? null : l.oldLine,
    new_line: l.type === 'del' ? null : l.newLine,
  });

  if (versions.isLoading || (versionId === 'latest' ? latest.isLoading : older.isLoading)) return <Loading />;
  const err = latest.error || older.error;
  if (err) return <ErrorBox error={err} />;
  if (!files?.length) return <Empty title="No changes between the source and target branches." />;
  const ctx = { project, mr, noteAwards };
  const total = files.length;
  const deleteDraft = async (id: number) => {
    try {
      await api.del(`${base}/draft_notes/${id}`);
      refresh();
    } catch (e) {
      toast(errorText(e), true);
    }
  };
  return (
    <>
      <div className="row wrap" style={{ marginBottom: 12 }}>
        <span>
          Compare <span className="branch-ref">{mr.target_branch}</span> and
        </span>
        <select style={{ width: 'auto' }} value={versionId} onChange={(e) => setVersionId(e.target.value === 'latest' ? 'latest' : Number(e.target.value))}>
          <option value="latest">latest version</option>
          {versions.data?.slice(1).map((v, i, all) => (
            <option key={v.id} value={v.id}>
              version {all.length - i} ({v.head_commit_sha.slice(0, 8)})
            </option>
          ))}
        </select>
        <span className="subtle small">
          {total} file{total === 1 ? '' : 's'} changed
        </span>
        <span className="spacer" />
        <label className="row small">
          <input type="checkbox" checked={showComments} onChange={(e) => setShowComments(e.target.checked)} /> Show comments
        </label>
      </div>
      <div className="diff-layout">
        <FileTree files={files} />
        <div className="diff-files">
          {files.map((f) => (
            <DiffFile
              key={fileKey(f)}
              file={f}
              comments={{
                onAddComment: user && refs && mr.state !== 'merged' ? (l) => setOpen({ file: fileKey(f), line: lineKey(l.oldLine, l.newLine) }) : undefined,
                renderBelow: (l) => {
                  const k = lineKey(l.type === 'add' ? null : l.oldLine, l.type === 'del' ? null : l.newLine);
                  const entry = byLine.get(`${f.new_path}|${k}`);
                  const isOpen = open?.file === fileKey(f) && open.line === lineKey(l.oldLine, l.newLine);
                  if (!isOpen && (!entry || !showComments || (!entry.threads.length && !entry.drafts.length))) return null;
                  return (
                    <div>
                      {showComments &&
                        entry?.threads.map((d) => (
                          <div key={d.id} style={{ marginBottom: 8 }}>
                            <Thread discussion={d} ctx={ctx} collapsedByDefault />
                          </div>
                        ))}
                      {showComments &&
                        entry?.drafts.map((d) => (
                          <div key={d.id} className="note-body-wrap" style={{ marginBottom: 8 }}>
                            <div className="note">
                              <div className="note-header">
                                <span className="badge draft-badge">Pending</span>
                                <span className="author-name">{user?.name}</span>
                                <span className="actions">
                                  <button className="btn btn-tertiary btn-sm btn-icon" title="Delete pending comment" onClick={() => deleteDraft(d.id)}>
                                    <Icon name="trash" size={14} />
                                  </button>
                                </span>
                              </div>
                              <Markdown source={d.note} projectPath={project.path_with_namespace} />
                            </div>
                          </div>
                        ))}
                      {isOpen && (
                        <div className="note-body-wrap" style={{ padding: 12 }}>
                          <CommentForm
                            projectPath={project.path_with_namespace}
                            autoFocus
                            insertSuggestion={l.type !== 'del' ? l.text : undefined}
                            onCancel={() => setOpen(null)}
                            actions={[
                              {
                                label: drafts.length ? 'Add to review' : 'Start a review',
                                run: async (body) => {
                                  await api.post(`${base}/draft_notes`, { note: body, position: position(f, l) });
                                  setOpen(null);
                                  refresh();
                                },
                              },
                              {
                                label: 'Add comment now',
                                kind: 'default',
                                run: async (body) => {
                                  await api.post(`${base}/discussions`, { body, position: position(f, l) });
                                  setOpen(null);
                                  refresh();
                                },
                              },
                            ]}
                          />
                        </div>
                      )}
                    </div>
                  );
                },
              }}
            />
          ))}
        </div>
      </div>
    </>
  );
}
