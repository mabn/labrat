// "Generate demo data" in the sidebar: adds demo projects, merge requests and users through
// POST /-/labrat/demo, after a confirmation.

import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { labrat } from '../api/client';
import { useEdition } from '../edition';
import { Icon } from './Icon';
import { ErrorBox, Modal, useToast } from './ui';

interface DemoResult {
  group: string;
  projects: { path: string; merge_requests: number }[];
  users: string[];
  password: string;
}

export function DemoDataButton() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="nav-item" onClick={() => setOpen(true)}>
        <Icon name="rocket" /> Generate demo data
      </button>
      {open && <DemoDataModal onClose={() => setOpen(false)} />}
    </>
  );
}

function DemoDataModal({ onClose }: { onClose: () => void }) {
  const { ee } = useEdition();
  const qc = useQueryClient();
  const toast = useToast();
  const navigate = useNavigate();
  const generate = useMutation({
    mutationFn: () => labrat<DemoResult>('POST', '/demo'),
    onSuccess: (r) => {
      qc.invalidateQueries();
      onClose();
      toast(`Demo data added: ${r.projects.length} projects`);
      navigate(`/${r.group}`);
    },
  });
  return (
    <Modal
      title="Generate demo data"
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={generate.isPending}>
            Cancel
          </button>
          <button className="btn btn-confirm" onClick={() => generate.mutate()} disabled={generate.isPending}>
            {generate.isPending ? 'Generating…' : 'Generate'}
          </button>
        </>
      }
    >
      <ErrorBox error={generate.error} />
      <p>
        This adds a <strong>demo</strong> group with a few projects. Each merge request shows one feature, and its title and
        description say which:
      </p>
      <ul style={{ margin: '8px 0 8px 20px' }}>
        <li>
          <strong>demo/web-app</strong>: reviews, suggestions, threads, drafts, conflicts, merged and closed merge requests
        </li>
        <li>
          <strong>demo/deploy-service</strong>: pipelines from the fake CI{ee !== false && ', and merge trains'}
        </li>
        {ee !== false && (
          <>
            <li>
              <strong>demo/api-service</strong>: approval rules, code owners and dependencies
            </li>
            <li>
              <strong>demo/payments</strong>: approval policies
            </li>
          </>
        )}
      </ul>
      {ee === false && <p className="subtle">labrat runs as CE (--noee), so the EE projects (approvals, approval policies and merge trains) are left out.</p>}
      <p>
        It also creates the users alice, bob, carol, dave, erin and frank, with the password <code>demo-password-123</code>. Some
        merge requests ask root for a review.
      </p>
      <p className="subtle">Existing data isn't changed. To generate the demo data again, reset labrat first.</p>
    </Modal>
  );
}
