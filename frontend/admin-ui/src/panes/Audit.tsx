import { useEffect, useState } from 'react';
import type { AuditEntryView } from '@xiaoguai/shared';
import { client } from '../client';

/**
 * v0.6.4: live audit log pane. Requires a tenant id; the chain is
 * per-tenant. We default to "ten_dev" for the unauthed dev mode and let
 * the operator override via the input box.
 */
export function AuditPane() {
  const [tenantId, setTenantId] = useState('ten_dev');
  const [rows, setRows] = useState<AuditEntryView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  async function load(tid: string): Promise<void> {
    setLoading(true);
    setError(null);
    try {
      const got = await client.listAudit({ tenant_id: tid, limit: 100 });
      setRows(got);
    } catch (err) {
      setRows(null);
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void load(tenantId);
    // Intentionally only on mount; further reloads happen on Refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <>
      <h1>Audit Log</h1>
      <div className="toolbar">
        <label>
          Tenant ID
          <input
            value={tenantId}
            onChange={(e) => setTenantId(e.target.value)}
            placeholder="ten_dev"
          />
        </label>
        <button onClick={() => void load(tenantId)} disabled={loading || !tenantId}>
          {loading ? 'Loading…' : 'Refresh'}
        </button>
      </div>

      {error && <div className="error">Failed: {error}</div>}

      {rows && rows.length === 0 && (
        <div className="empty">No audit rows for tenant <code>{tenantId}</code>.</div>
      )}

      {rows && rows.length > 0 && (
        <table className="audit-table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Timestamp</th>
              <th>Actor</th>
              <th>Action</th>
              <th>Resource</th>
              <th>HMAC (last 8)</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id}>
                <td>{r.id}</td>
                <td>{new Date(r.ts).toLocaleString()}</td>
                <td>{r.actor}</td>
                <td>{r.action}</td>
                <td>{r.resource ?? '-'}</td>
                <td className="mono">…{r.hmac.slice(-8)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}
