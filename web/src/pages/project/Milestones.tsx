import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Milestone } from '../../api/types';
import { Empty, ErrorBox, Loading, Markdown, Modal } from '../../components/ui';
import { useProject, useProjectCrumbs } from './ProjectRoutes';

export function Milestones() {
  const { project, canWrite } = useProject();
  useProjectCrumbs([{ label: 'Milestones' }]);
  const qc = useQueryClient();
  const [state, setState] = useState('active');
  const [editing, setEditing] = useState<Milestone | 'new' | null>(null);
  const base = `/projects/${project.id}/milestones`;
  const q = useQuery({ queryKey: ['milestones', project.id, state], queryFn: () => api.get<Milestone[]>(base, { state: state === 'all' ? undefined : state, per_page: 100, include_ancestors: true }) });
  const toggle = useMutation({
    mutationFn: (m: Milestone) => api.put(`${base}/${m.id}`, { state_event: m.state === 'active' ? 'close' : 'activate' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['milestones', project.id] }),
  });
  return (
    <>
      <div className="page-head">
        <h1>Milestones</h1>
        <div className="actions">
          {canWrite && (
            <button className="btn btn-confirm" onClick={() => setEditing('new')}>
              New milestone
            </button>
          )}
        </div>
      </div>
      <div className="tabs">
        {[
          ['active', 'Open'],
          ['closed', 'Closed'],
          ['all', 'All'],
        ].map(([s, l]) => (
          <button key={s} className={`tab${state === s ? ' active' : ''}`} onClick={() => setState(s)}>
            {l}
          </button>
        ))}
      </div>
      <ErrorBox error={q.error || toggle.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data?.length ? (
        <ul className="list list-bordered">
          {q.data.map((m) => (
            <li key={m.id}>
              <div className="grow">
                <div className="title">{m.title}</div>
                <div className="meta">
                  {m.start_date && `${m.start_date} – `}
                  {m.due_date ?? 'No due date'} {m.group_id && '· Group milestone'}
                </div>
                {m.description && <Markdown source={m.description} className="small" />}
              </div>
              <span className={`badge ${m.state === 'active' ? 'badge-success' : 'badge-neutral'}`}>{m.state === 'active' ? 'Open' : 'Closed'}</span>
              {canWrite && m.project_id && (
                <>
                  <button className="btn btn-sm" onClick={() => setEditing(m)}>
                    Edit
                  </button>
                  <button className="btn btn-sm" onClick={() => toggle.mutate(m)}>
                    {m.state === 'active' ? 'Close' : 'Reopen'}
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="Use milestones to track merge requests over a fixed period of time" />
      )}
      {editing && <MilestoneForm base={base} milestone={editing === 'new' ? undefined : editing} onClose={() => setEditing(null)} />}
    </>
  );
}

function MilestoneForm({ base, milestone, onClose }: { base: string; milestone?: Milestone; onClose: () => void }) {
  const { project } = useProject();
  const qc = useQueryClient();
  const [title, setTitle] = useState(milestone?.title ?? '');
  const [description, setDescription] = useState(milestone?.description ?? '');
  const [start, setStart] = useState(milestone?.start_date ?? '');
  const [due, setDue] = useState(milestone?.due_date ?? '');
  const save = useMutation({
    mutationFn: () => {
      const body = { title, description, start_date: start || undefined, due_date: due || undefined };
      return milestone ? api.put(`${base}/${milestone.id}`, body) : api.post(base, body);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['milestones', project.id] });
      onClose();
    },
  });
  return (
    <Modal
      title={milestone ? 'Edit milestone' : 'New milestone'}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-confirm" disabled={!title || save.isPending} onClick={() => save.mutate()}>
            {milestone ? 'Save changes' : 'Create milestone'}
          </button>
        </>
      }
    >
      <ErrorBox error={save.error} />
      <label className="field">
        <span className="label">Title</span>
        <input type="text" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
      </label>
      <div className="row">
        <label className="field grow">
          <span className="label">Start date</span>
          <input type="date" value={start} onChange={(e) => setStart(e.target.value)} />
        </label>
        <label className="field grow">
          <span className="label">Due date</span>
          <input type="date" value={due} onChange={(e) => setDue(e.target.value)} />
        </label>
      </div>
      <label className="field">
        <span className="label">Description</span>
        <textarea rows={4} value={description} onChange={(e) => setDescription(e.target.value)} />
      </label>
    </Modal>
  );
}
