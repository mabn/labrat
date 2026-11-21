import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Approvals, DraftNote, MergeRequest, Project } from '../../api/types';
import { ErrorBox, Markdown, Modal, useToast } from '../../components/ui';
import { mrBase, useMrRefresh } from './data';

/** The bar shown while the user has pending (draft) comments, to submit or discard the review. */
export function ReviewBar({ project, mr, drafts, approvals }: { project: Project; mr: MergeRequest; drafts: DraftNote[]; approvals?: Approvals }) {
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const [submitting, setSubmitting] = useState(false);
  const [listing, setListing] = useState(false);
  const [summary, setSummary] = useState('');
  const [approve, setApprove] = useState(false);
  const submit = useMutation({
    mutationFn: async () => {
      await api.post(`${base}/draft_notes/bulk_publish`);
      if (summary.trim()) await api.post(`${base}/notes`, { body: summary });
      if (approve) await api.post(`${base}/approve`, { sha: mr.sha });
    },
    onSuccess: () => {
      setSubmitting(false);
      setSummary('');
      setApprove(false);
      toast('Review submitted');
      refresh();
    },
  });
  const discard = useMutation({
    mutationFn: async () => {
      for (const d of drafts) await api.del(`${base}/draft_notes/${d.id}`);
    },
    onSuccess: refresh,
  });
  if (!drafts.length) return null;
  return (
    <>
      <div className="review-bar">
        <button className="btn btn-tertiary" onClick={() => setListing(true)}>
          <strong>{drafts.length}</strong>&nbsp;pending comment{drafts.length === 1 ? '' : 's'}
        </button>
        <span className="spacer" />
        <button className="btn" disabled={discard.isPending} onClick={() => confirm('Discard all pending comments?') && discard.mutate()}>
          Discard review
        </button>
        <button className="btn btn-confirm" onClick={() => setSubmitting(true)}>
          Submit review
        </button>
      </div>
      {listing && (
        <Modal title="Pending comments" onClose={() => setListing(false)}>
          <ul className="list list-bordered">
            {drafts.map((d) => (
              <li key={d.id} style={{ display: 'block' }}>
                <div className="small subtle mono" style={{ marginBottom: 4 }}>
                  {d.position?.new_path ? `${d.position.new_path}:${d.position.new_line ?? d.position.old_line}` : d.discussion_id ? 'Reply to a thread' : 'Overview'}
                </div>
                <Markdown source={d.note} projectPath={project.path_with_namespace} />
                <button
                  className="btn btn-sm"
                  style={{ marginTop: 6 }}
                  onClick={() =>
                    api
                      .put(`${base}/draft_notes/${d.id}/publish`)
                      .then(refresh)
                      .catch((e) => toast(String(e.message), true))
                  }
                >
                  Publish now
                </button>
              </li>
            ))}
          </ul>
        </Modal>
      )}
      {submitting && (
        <Modal
          title="Submit your review"
          onClose={() => setSubmitting(false)}
          footer={
            <>
              <button className="btn" onClick={() => setSubmitting(false)}>
                Cancel
              </button>
              <button className="btn btn-confirm" disabled={submit.isPending} onClick={() => submit.mutate()}>
                Submit review
              </button>
            </>
          }
        >
          <ErrorBox error={submit.error} />
          <label className="field">
            <span className="label">Summary comment (optional)</span>
            <textarea rows={4} value={summary} onChange={(e) => setSummary(e.target.value)} placeholder="Write a summary of your review" />
          </label>
          {approvals?.user_can_approve && (
            <label className="check">
              <input type="checkbox" checked={approve} onChange={(e) => setApprove(e.target.checked)} /> Approve merge request
            </label>
          )}
          <p className="subtle small">
            Publishes {drafts.length} pending comment{drafts.length === 1 ? '' : 's'}.
          </p>
        </Modal>
      )}
    </>
  );
}
