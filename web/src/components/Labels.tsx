import { useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Label } from '../api/types';
import { Empty, ErrorBox, LabelPill, Loading, Modal, useToast, errorText } from './ui';

const palette = ['#009966', '#8fbc8f', '#3cb371', '#00b140', '#013220', '#6699cc', '#0000ff', '#e6e6fa', '#9400d3', '#330066', '#808080', '#36454f', '#f7e7ce', '#c21e56', '#cc338b', '#dc143c', '#ff0000', '#cd5b45', '#eee600', '#ed9121', '#c39953'];

function LabelForm({ initial, onSave, onClose, busy, error }: { initial?: Label; onSave: (v: { name: string; color: string; description: string }) => void; onClose: () => void; busy: boolean; error: unknown }) {
  const [name, setName] = useState(initial?.name ?? '');
  const [color, setColor] = useState(initial?.color ?? palette[Math.floor(Math.random() * palette.length)]);
  const [description, setDescription] = useState(initial?.description ?? '');
  return (
    <Modal
      title={initial ? 'Edit label' : 'New label'}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-confirm" disabled={!name || busy} onClick={() => onSave({ name, color, description })}>
            {initial ? 'Save changes' : 'Create label'}
          </button>
        </>
      }
    >
      <ErrorBox error={error} />
      <label className="field">
        <span className="label">Title</span>
        <input type="text" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field">
        <span className="label">Description</span>
        <input type="text" value={description} onChange={(e) => setDescription(e.target.value)} />
      </label>
      <div className="field">
        <span className="label-text">Background color</span>
        <div className="row wrap" style={{ gap: 4, marginBottom: 8 }}>
          {palette.map((c) => (
            <button key={c} type="button" onClick={() => setColor(c)} title={c} style={{ width: 24, height: 24, background: c, border: c === color ? '2px solid #000' : '1px solid #ccc', borderRadius: 4, cursor: 'pointer' }} />
          ))}
        </div>
        <input type="text" value={color} onChange={(e) => setColor(e.target.value)} style={{ width: 120 }} />
      </div>
      {name && <LabelPill label={{ id: 0, name, color, text_color: '#fff', description }} />}
    </Modal>
  );
}

/** Labels of a project or group (`base` is `/projects/:id` or `/groups/:id`). */
export function Labels({ base, canManage }: { base: string; canManage: boolean }) {
  const qc = useQueryClient();
  const toast = useToast();
  const key = ['labels', base];
  const q = useQuery({ queryKey: key, queryFn: () => api.all<Label>(`${base}/labels`, { with_counts: true, include_ancestor_groups: true }) });
  const [editing, setEditing] = useState<Label | 'new' | null>(null);
  const save = useMutation({
    mutationFn: (v: { name: string; color: string; description: string }) =>
      editing === 'new'
        ? api.post(`${base}/labels`, v)
        : api.put(`${base}/labels/${encodeURIComponent(String((editing as Label).id))}`, { new_name: v.name, color: v.color, description: v.description }),
    onSuccess: () => {
      setEditing(null);
      qc.invalidateQueries({ queryKey: key });
    },
  });
  const remove = useMutation({
    mutationFn: (l: Label) => api.del(`${base}/labels/${l.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: key }),
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <>
      <div className="page-head">
        <h1>Labels</h1>
        <div className="actions">
          {canManage && (
            <button className="btn btn-confirm" onClick={() => setEditing('new')}>
              New label
            </button>
          )}
        </div>
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data?.length ? (
        <ul className="list list-bordered">
          {q.data.map((l) => (
            <li key={l.id} style={{ alignItems: 'center' }}>
              <div style={{ width: 220 }}>
                <LabelPill label={l} />
              </div>
              <div className="grow subtle">{l.description}</div>
              <span className="small subtle">{l.open_merge_requests_count ?? 0} open merge requests</span>
              {canManage && (l.is_project_label !== false || base.startsWith('/groups')) && (
                <>
                  <button className="btn btn-sm" onClick={() => setEditing(l)}>
                    Edit
                  </button>
                  <button className="btn btn-sm btn-danger-secondary" onClick={() => confirm(`Delete label ${l.name}?`) && remove.mutate(l)}>
                    Delete
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      ) : (
        <Empty title="Use labels to divide merge requests into categories" />
      )}
      {editing && <LabelForm initial={editing === 'new' ? undefined : editing} onClose={() => setEditing(null)} onSave={(v) => save.mutate(v)} busy={save.isPending} error={save.error} />}
    </>
  );
}
