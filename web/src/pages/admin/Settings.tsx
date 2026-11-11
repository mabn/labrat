import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import { Layout, useCrumbs } from '../../components/Layout';
import { ErrorBox, Loading, useToast } from '../../components/ui';

interface Settings {
  allow_local_requests_from_web_hooks_and_services: boolean;
  allow_local_requests_from_system_hooks: boolean;
}

export function AdminSettings() {
  useCrumbs([{ label: 'Admin area' }, { label: 'Settings' }]);
  const qc = useQueryClient();
  const toast = useToast();
  const q = useQuery({ queryKey: ['application-settings'], queryFn: () => api.get<Settings>('/application/settings') });
  const [form, setForm] = useState<Settings | null>(null);
  useEffect(() => {
    if (q.data) setForm(q.data);
  }, [q.data]);
  const save = useMutation({
    mutationFn: (body: Partial<Settings>) => api.put<Settings>('/application/settings', body),
    onSuccess: () => {
      toast('Application settings saved');
      qc.invalidateQueries({ queryKey: ['application-settings'] });
    },
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>Network settings</h1>
      </div>
      <ErrorBox error={q.error || save.error} />
      {!form ? (
        <Loading />
      ) : (
        <div className="form-narrow">
          <h3 style={{ marginBottom: 8 }}>Outbound requests</h3>
          <label className="check">
            <input type="checkbox" checked={form.allow_local_requests_from_web_hooks_and_services} onChange={(e) => setForm({ ...form, allow_local_requests_from_web_hooks_and_services: e.target.checked })} />
            <span>
              Allow requests to the local network from webhooks and integrations
              <div className="help">Needed for webhooks that point at localhost or a private address, as test receivers usually do.</div>
            </span>
          </label>
          <label className="check">
            <input type="checkbox" checked={form.allow_local_requests_from_system_hooks} onChange={(e) => setForm({ ...form, allow_local_requests_from_system_hooks: e.target.checked })} />
            <span>Allow requests to the local network from system hooks</span>
          </label>
          <button className="btn btn-confirm" style={{ marginTop: 12 }} disabled={save.isPending} onClick={() => save.mutate(form)}>
            Save changes
          </button>
        </div>
      )}
    </Layout>
  );
}
