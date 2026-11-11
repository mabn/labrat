import { Layout, useCrumbs } from '../../components/Layout';
import { Webhooks } from '../../components/Webhooks';

export function SystemHooks() {
  useCrumbs([{ label: 'Admin area' }, { label: 'System hooks' }]);
  return (
    <Layout>
      <Webhooks base="" />
    </Layout>
  );
}
