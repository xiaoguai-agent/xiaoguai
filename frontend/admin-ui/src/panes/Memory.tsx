/**
 * v1.4-ready — Memory browser / editor (ADR-0019).
 *
 * The `xiaoguai-memory` Rust crate (task #155) is not yet shipped.
 * When /v1/memory/* returns 404 this pane renders a banner and falls
 * back to mock data so operators can validate the UI design in advance.
 *
 * Three tabs:
 *   1. List      — filterable table by type / tenant / agent / tag / time
 *   2. Recall    — trace which memories were recalled for a session/query
 *   3. Neighbors — vector-similarity neighborhood for a given memory
 */

import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  MemoryRecord,
  MemoryType,
  RecallEntry,
  RecallTraceResponse,
  SimilarMemory,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { client } from '../client';
import { MemoryImportExport } from './MemoryImportExport';

// ---------------------------------------------------------------------------
// Mock data (shown when /v1/memory/* returns 404)
// ---------------------------------------------------------------------------

const MOCK_MEMORIES: MemoryRecord[] = [
  {
    id: 'mem_mock_001',
    type: 'fact',
    content:
      'The primary data center is located in Frankfurt (eu-central-1). All PII must stay within the EU.',
    tags: ['infra', 'compliance'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_ops',
    created_at: '2026-05-01T09:00:00Z',
    last_recalled_at: '2026-05-24T14:30:00Z',
    recall_count: 12,
    ttl: null,
  },
  {
    id: 'mem_mock_002',
    type: 'fact',
    content:
      'SLA for Tier-1 incidents is 15 minutes acknowledgment, 4 hours resolution. Escalate to on-call VP after 2 hours.',
    tags: ['sla', 'incident'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_ops',
    created_at: '2026-05-03T10:15:00Z',
    last_recalled_at: '2026-05-22T08:10:00Z',
    recall_count: 7,
    ttl: null,
  },
  {
    id: 'mem_mock_003',
    type: 'fact',
    content:
      'PostgreSQL version in production is 16.3. The read replica lag threshold is 30 seconds before alert fires.',
    tags: ['database', 'alerts'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_dba',
    created_at: '2026-05-10T11:00:00Z',
    last_recalled_at: null,
    recall_count: 0,
    ttl: 'P90D',
  },
  {
    id: 'mem_mock_004',
    type: 'episode',
    content:
      'Incident 2026-05-18: disk full on kafka-03 caused 6-minute message lag. Root cause: log rotation misconfigured after the May maintenance window.',
    tags: ['incident', 'kafka', 'postmortem'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_ops',
    created_at: '2026-05-18T23:45:00Z',
    last_recalled_at: '2026-05-19T09:00:00Z',
    recall_count: 3,
    ttl: 'P365D',
  },
  {
    id: 'mem_mock_005',
    type: 'episode',
    content:
      'On-boarding session with customer Acme Corp (2026-05-20). They need SSO via SAML and a dedicated EU tenant. Follow-up call scheduled for 2026-06-03.',
    tags: ['customer', 'onboarding', 'acme'],
    tenant_id: 'ten_demo',
    agent_id: 'agent_sales',
    created_at: '2026-05-20T15:00:00Z',
    last_recalled_at: '2026-05-21T08:30:00Z',
    recall_count: 2,
    ttl: 'P180D',
  },
  {
    id: 'mem_mock_006',
    type: 'preference',
    content:
      'User boyi.liang@example.com prefers concise bullet-point summaries. Always include TL;DR at the top. Avoid jargon.',
    tags: ['user-pref', 'formatting'],
    tenant_id: 'ten_demo',
    agent_id: null,
    created_at: '2026-05-05T08:00:00Z',
    last_recalled_at: '2026-05-25T10:00:00Z',
    recall_count: 28,
    ttl: null,
  },
];

const MOCK_RECALL_TRACE: RecallTraceResponse = {
  session_id: 'sess_mock_abc123',
  query: null,
  entries: [
    {
      memory_id: 'mem_mock_006',
      relevance_score: 0.97,
      agent_id: 'agent_ops',
      recalled_at: '2026-05-25T10:00:00Z',
      content_preview: 'User boyi.liang@example.com prefers concise bullet-point summaries…',
      type: 'preference',
      tags: ['user-pref', 'formatting'],
    },
    {
      memory_id: 'mem_mock_001',
      relevance_score: 0.82,
      agent_id: 'agent_ops',
      recalled_at: '2026-05-25T10:00:01Z',
      content_preview: 'The primary data center is located in Frankfurt (eu-central-1)…',
      type: 'fact',
      tags: ['infra', 'compliance'],
    },
    {
      memory_id: 'mem_mock_004',
      relevance_score: 0.71,
      agent_id: 'agent_ops',
      recalled_at: '2026-05-25T10:00:01Z',
      content_preview: 'Incident 2026-05-18: disk full on kafka-03 caused 6-minute message lag…',
      type: 'episode',
      tags: ['incident', 'kafka'],
    },
  ],
  total: 3,
};

const MOCK_SIMILAR: SimilarMemory[] = [
  {
    memory_id: 'mem_mock_002',
    similarity: 0.88,
    content_preview: 'SLA for Tier-1 incidents is 15 minutes acknowledgment…',
    type: 'fact',
    tags: ['sla', 'incident'],
    created_at: '2026-05-03T10:15:00Z',
  },
  {
    memory_id: 'mem_mock_004',
    similarity: 0.74,
    content_preview: 'Incident 2026-05-18: disk full on kafka-03…',
    type: 'episode',
    tags: ['incident', 'kafka'],
    created_at: '2026-05-18T23:45:00Z',
  },
];

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MEMORY_TYPES: MemoryType[] = ['fact', 'episode', 'preference'];

type TabId = 'list' | 'recall' | 'neighbors';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtDate(iso: string | null): string {
  if (!iso) return '—';
  return new Date(iso).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function preview(content: string): string {
  const lines = content.split('\n').slice(0, 2).join(' ');
  return lines.length > 120 ? `${lines.slice(0, 120)}…` : lines;
}

function typeBadgeClass(type: MemoryType): string {
  if (type === 'fact') return 'kind-tag kind-tag-chat';
  if (type === 'episode') return 'kind-tag kind-tag-scheduled';
  return 'kind-tag kind-tag-im';
}

// ---------------------------------------------------------------------------
// 404 banner
// ---------------------------------------------------------------------------

function NotReadyBanner(): JSX.Element {
  const { t } = useTranslation();
  return (
    <div
      className="error"
      role="status"
      style={{
        background: '#fffbeb',
        border: '1px solid #f59e0b',
        borderRadius: '6px',
        padding: '0.75rem 1rem',
        marginBottom: '1rem',
        color: '#92400e',
      }}
    >
      {t('pane.memory.not_ready_banner')}
    </div>
  );
}

// ---------------------------------------------------------------------------
// New / Edit memory modal
// ---------------------------------------------------------------------------

interface MemoryFormProps {
  tenantId: string;
  existing: MemoryRecord | null;
  onClose: () => void;
  onSaved: (record: MemoryRecord) => void;
  is404: boolean;
}

function MemoryModal({ tenantId, existing, onClose, onSaved, is404 }: MemoryFormProps): JSX.Element {
  const { t } = useTranslation();
  const isNew = existing === null;

  const [type, setType] = useState<MemoryType>(existing?.type ?? 'fact');
  const [content, setContent] = useState(existing?.content ?? '');
  const [tagsRaw, setTagsRaw] = useState((existing?.tags ?? []).join(', '));
  const [ttl, setTtl] = useState(existing?.ttl ?? '');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSave(): Promise<void> {
    setSaving(true);
    setError(null);
    const tags = tagsRaw
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    const ttlVal = ttl.trim() || null;

    try {
      let saved: MemoryRecord;
      if (is404) {
        // Mock: synthesize a fake response so the UI flows correctly.
        saved = {
          id: existing?.id ?? `mem_preview_${Date.now()}`,
          type,
          content,
          tags,
          tenant_id: tenantId || 'ten_demo',
          agent_id: existing?.agent_id ?? null,
          created_at: existing?.created_at ?? new Date().toISOString(),
          last_recalled_at: existing?.last_recalled_at ?? null,
          recall_count: existing?.recall_count ?? 0,
          ttl: ttlVal,
        };
      } else if (isNew) {
        saved = await client.createMemory({
          type,
          content,
          tags,
          tenant_id: tenantId,
          ttl: ttlVal,
        });
      } else {
        saved = await client.updateMemory(existing!.id, { content, tags, ttl: ttlVal });
      }
      onSaved(saved);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={isNew ? t('pane.memory.modal_title_new') : t('pane.memory.modal_title_edit')}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,0.4)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        zIndex: 1000,
      }}
    >
      <div
        style={{
          background: 'var(--bg, #fff)',
          border: '1px solid var(--border, #e5e7eb)',
          borderRadius: '8px',
          padding: '1.5rem',
          width: 'min(480px, 90vw)',
          maxHeight: '90vh',
          overflowY: 'auto',
        }}
      >
        <h2 style={{ margin: '0 0 1rem', fontSize: '1.1rem' }}>
          {isNew ? t('pane.memory.modal_title_new') : t('pane.memory.modal_title_edit')}
        </h2>

        {error && <div className="error">{t('common.failed', { message: error })}</div>}

        <div style={{ display: 'flex', flexDirection: 'column', gap: '0.75rem' }}>
          <label>
            {t('pane.memory.field_type')}
            <select
              value={type}
              onChange={(e) => setType(e.target.value as MemoryType)}
              disabled={!isNew}
              style={{ marginLeft: '0.5rem' }}
            >
              {MEMORY_TYPES.map((mt) => (
                <option key={mt} value={mt}>
                  {t(`pane.memory.type_${mt}`)}
                </option>
              ))}
            </select>
          </label>

          <label style={{ display: 'flex', flexDirection: 'column', gap: '0.25rem' }}>
            {t('pane.memory.field_content')}
            <textarea
              value={content}
              onChange={(e) => setContent(e.target.value)}
              rows={6}
              style={{ fontFamily: 'monospace', fontSize: '0.85rem', resize: 'vertical' }}
              placeholder={t('pane.memory.placeholder_content')}
            />
          </label>

          <label>
            {t('pane.memory.field_tags')}
            <input
              value={tagsRaw}
              onChange={(e) => setTagsRaw(e.target.value)}
              placeholder={t('pane.memory.placeholder_tags')}
              style={{ marginLeft: '0.5rem', width: '100%' }}
            />
          </label>

          <label>
            {t('pane.memory.field_ttl')}
            <input
              value={ttl}
              onChange={(e) => setTtl(e.target.value)}
              placeholder={t('pane.memory.placeholder_ttl')}
              style={{ marginLeft: '0.5rem', width: '60%' }}
            />
          </label>
        </div>

        <div style={{ marginTop: '1.25rem', display: 'flex', gap: '0.5rem', justifyContent: 'flex-end' }}>
          <button onClick={onClose}>{t('common.close')}</button>
          <button onClick={() => void handleSave()} disabled={saving || !content.trim()}>
            {saving ? t('common.loading') : t('pane.memory.btn_save')}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Delete confirmation modal
// ---------------------------------------------------------------------------

interface DeleteConfirmProps {
  record: MemoryRecord;
  onCancel: () => void;
  onConfirmed: () => void;
  is404: boolean;
}

function DeleteConfirmModal({ record, onCancel, onConfirmed, is404 }: DeleteConfirmProps): JSX.Element {
  const { t } = useTranslation();
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleDelete(): Promise<void> {
    setDeleting(true);
    setError(null);
    try {
      if (!is404) {
        await client.deleteMemory(record.id);
      }
      onConfirmed();
    } catch (e) {
      setError((e as Error).message);
      setDeleting(false);
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={t('pane.memory.delete_confirm_title')}
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(0,0,0,0.4)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        zIndex: 1000,
      }}
    >
      <div
        style={{
          background: 'var(--bg, #fff)',
          border: '1px solid var(--border, #e5e7eb)',
          borderRadius: '8px',
          padding: '1.5rem',
          width: 'min(400px, 90vw)',
        }}
      >
        <h2 style={{ margin: '0 0 0.75rem', fontSize: '1rem' }}>
          {t('pane.memory.delete_confirm_title')}
        </h2>
        <p style={{ marginBottom: '0.5rem', fontSize: '0.9rem', color: '#374151' }}>
          {t('pane.memory.delete_confirm_body')}
        </p>
        <code
          style={{
            display: 'block',
            background: '#f3f4f6',
            padding: '0.5rem',
            borderRadius: '4px',
            fontSize: '0.8rem',
            marginBottom: '0.75rem',
            wordBreak: 'break-all',
          }}
        >
          {record.id}
        </code>
        {error && <div className="error">{t('common.failed', { message: error })}</div>}
        <div style={{ display: 'flex', gap: '0.5rem', justifyContent: 'flex-end' }}>
          <button onClick={onCancel}>{t('common.close')}</button>
          <button
            onClick={() => void handleDelete()}
            disabled={deleting}
            style={{ background: '#ef4444', color: '#fff', border: 'none', borderRadius: '4px', padding: '0.3rem 0.75rem', cursor: 'pointer' }}
          >
            {deleting ? t('common.loading') : t('pane.memory.btn_delete_confirm')}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tab 1: List view
// ---------------------------------------------------------------------------

interface ListViewProps {
  is404: boolean;
}

function ListView({ is404 }: ListViewProps): JSX.Element {
  const { t } = useTranslation();

  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Filters
  const [filterType, setFilterType] = useState<MemoryType | ''>('');
  const [filterTenant, setFilterTenant] = useState('');
  const [filterAgent, setFilterAgent] = useState('');
  const [filterTag, setFilterTag] = useState('');
  const [filterSince, setFilterSince] = useState('');
  const [filterUntil, setFilterUntil] = useState('');

  // Modals
  const [editingRecord, setEditingRecord] = useState<MemoryRecord | null | undefined>(undefined);
  // undefined = closed, null = new, MemoryRecord = edit existing
  const [deletingRecord, setDeletingRecord] = useState<MemoryRecord | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      if (is404) {
        let filtered = MOCK_MEMORIES;
        if (filterType) filtered = filtered.filter((r) => r.type === filterType);
        if (filterTenant) filtered = filtered.filter((r) => r.tenant_id.includes(filterTenant));
        if (filterAgent) filtered = filtered.filter((r) => r.agent_id?.includes(filterAgent));
        if (filterTag) filtered = filtered.filter((r) => r.tags.some((tg) => tg.includes(filterTag)));
        setRecords(filtered);
        setTotal(filtered.length);
      } else {
        const resp = await client.listMemories({
          type: filterType || undefined,
          tenant_id: filterTenant || undefined,
          agent_id: filterAgent || undefined,
          tag: filterTag || undefined,
          since: filterSince || undefined,
          until: filterUntil || undefined,
          limit: 50,
        });
        setRecords(resp.records);
        setTotal(resp.total);
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [is404, filterType, filterTenant, filterAgent, filterTag, filterSince, filterUntil]);

  useEffect(() => {
    void load();
  }, [load]);

  function handleSaved(saved: MemoryRecord): void {
    setRecords((prev) => {
      const idx = prev.findIndex((r) => r.id === saved.id);
      if (idx === -1) return [saved, ...prev];
      return prev.map((r) => (r.id === saved.id ? saved : r));
    });
    setEditingRecord(undefined);
  }

  function handleDeleted(id: string): void {
    setRecords((prev) => prev.filter((r) => r.id !== id));
    setTotal((t) => Math.max(0, t - 1));
    setDeletingRecord(null);
  }

  const tenantForNew = filterTenant || 'ten_dev';

  return (
    <>
      {/* Filters */}
      <div className="today-filters" role="group" aria-label={t('pane.memory.filters_aria')}>
        <label>
          {t('pane.memory.filter_type')}
          <select
            value={filterType}
            onChange={(e) => setFilterType(e.target.value as MemoryType | '')}
            style={{ marginLeft: '0.4rem' }}
          >
            <option value="">{t('pane.memory.type_all')}</option>
            {MEMORY_TYPES.map((mt) => (
              <option key={mt} value={mt}>
                {t(`pane.memory.type_${mt}`)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t('pane.memory.filter_tenant')}
          <input
            value={filterTenant}
            onChange={(e) => setFilterTenant(e.target.value)}
            className="search"
            placeholder="ten_dev"
            style={{ marginLeft: '0.4rem', width: '9rem' }}
          />
        </label>
        <label>
          {t('pane.memory.filter_agent')}
          <input
            value={filterAgent}
            onChange={(e) => setFilterAgent(e.target.value)}
            className="search"
            placeholder="agent_id"
            style={{ marginLeft: '0.4rem', width: '9rem' }}
          />
        </label>
        <label>
          {t('pane.memory.filter_tag')}
          <input
            value={filterTag}
            onChange={(e) => setFilterTag(e.target.value)}
            className="search"
            placeholder="tag"
            style={{ marginLeft: '0.4rem', width: '7rem' }}
          />
        </label>
        <label>
          {t('pane.memory.filter_since')}
          <input
            type="date"
            value={filterSince}
            onChange={(e) => setFilterSince(e.target.value)}
            style={{ marginLeft: '0.4rem' }}
          />
        </label>
        <label>
          {t('pane.memory.filter_until')}
          <input
            type="date"
            value={filterUntil}
            onChange={(e) => setFilterUntil(e.target.value)}
            style={{ marginLeft: '0.4rem' }}
          />
        </label>
        <button onClick={() => void load()} disabled={loading}>
          {loading ? t('common.loading') : t('common.refresh')}
        </button>
        <button
          onClick={() => setEditingRecord(null)}
          style={{ marginLeft: '0.5rem' }}
        >
          {t('pane.memory.btn_new')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      <p style={{ fontSize: '0.8rem', color: '#6b7280', margin: '0.5rem 0 0.75rem' }}>
        {t('pane.memory.list_count', { count: records.length, total })}
      </p>

      {records.length === 0 && !loading && (
        <div className="empty">{t('pane.memory.list_empty')}</div>
      )}

      {records.length > 0 && (
        <table className="usage-table">
          <thead>
            <tr>
              <th scope="col">{t('pane.memory.col_type')}</th>
              <th scope="col">{t('pane.memory.col_preview')}</th>
              <th scope="col">{t('pane.memory.col_tags')}</th>
              <th scope="col">{t('pane.memory.col_created')}</th>
              <th scope="col">{t('pane.memory.col_last_recalled')}</th>
              <th scope="col">{t('pane.memory.col_recall_count')}</th>
              <th scope="col">{t('pane.memory.col_actions')}</th>
            </tr>
          </thead>
          <tbody>
            {records.map((rec) => (
              <tr key={rec.id}>
                <td>
                  <span className={typeBadgeClass(rec.type)}>
                    {t(`pane.memory.type_${rec.type}`)}
                  </span>
                </td>
                <td style={{ maxWidth: '28rem', fontSize: '0.8rem', color: '#374151' }}>
                  {preview(rec.content)}
                </td>
                <td style={{ fontSize: '0.78rem' }}>
                  {rec.tags.map((tag) => (
                    <span
                      key={tag}
                      className="kind-tag"
                      style={{ marginRight: '0.25rem', opacity: 0.75 }}
                    >
                      {tag}
                    </span>
                  ))}
                </td>
                <td style={{ whiteSpace: 'nowrap', fontSize: '0.8rem' }}>
                  {fmtDate(rec.created_at)}
                </td>
                <td style={{ whiteSpace: 'nowrap', fontSize: '0.8rem' }}>
                  {fmtDate(rec.last_recalled_at)}
                </td>
                <td style={{ textAlign: 'right', fontSize: '0.8rem' }}>{rec.recall_count}</td>
                <td>
                  <button
                    style={{ marginRight: '0.3rem', fontSize: '0.8rem' }}
                    onClick={() => setEditingRecord(rec)}
                    aria-label={`${t('pane.memory.btn_edit')} ${rec.id}`}
                  >
                    {t('pane.memory.btn_edit')}
                  </button>
                  <button
                    style={{ fontSize: '0.8rem', color: '#dc2626' }}
                    onClick={() => setDeletingRecord(rec)}
                    aria-label={`${t('pane.memory.btn_delete')} ${rec.id}`}
                  >
                    {t('pane.memory.btn_delete')}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {editingRecord !== undefined && (
        <MemoryModal
          tenantId={tenantForNew}
          existing={editingRecord}
          onClose={() => setEditingRecord(undefined)}
          onSaved={handleSaved}
          is404={is404}
        />
      )}

      {deletingRecord && (
        <DeleteConfirmModal
          record={deletingRecord}
          onCancel={() => setDeletingRecord(null)}
          onConfirmed={() => handleDeleted(deletingRecord.id)}
          is404={is404}
        />
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Tab 2: Recall trace view
// ---------------------------------------------------------------------------

interface RecallViewProps {
  is404: boolean;
}

function RecallView({ is404 }: RecallViewProps): JSX.Element {
  const { t } = useTranslation();
  const [sessionId, setSessionId] = useState('');
  const [query, setQuery] = useState('');
  const [trace, setTrace] = useState<RecallTraceResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSearch(): Promise<void> {
    if (!sessionId.trim() && !query.trim()) return;
    setLoading(true);
    setError(null);
    try {
      if (is404) {
        setTrace(MOCK_RECALL_TRACE);
      } else {
        const resp = await client.recallMemoriesForSession({
          session_id: sessionId.trim() || undefined,
          query: query.trim() || undefined,
          limit: 20,
        });
        setTrace(resp);
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }

  return (
    <>
      <p style={{ fontSize: '0.85rem', color: '#6b7280', marginBottom: '0.75rem' }}>
        {t('pane.memory.recall_description')}
      </p>

      <div className="today-filters" role="group" aria-label={t('pane.memory.recall_filters_aria')}>
        <label>
          {t('pane.memory.recall_session_id')}
          <input
            value={sessionId}
            onChange={(e) => setSessionId(e.target.value)}
            className="search"
            placeholder="sess_abc123"
            style={{ marginLeft: '0.4rem', width: '14rem' }}
          />
        </label>
        <label>
          {t('pane.memory.recall_query')}
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="search"
            placeholder={t('pane.memory.recall_query_placeholder')}
            style={{ marginLeft: '0.4rem', width: '18rem' }}
          />
        </label>
        <button
          onClick={() => void handleSearch()}
          disabled={loading || (!sessionId.trim() && !query.trim())}
        >
          {loading ? t('common.loading') : t('pane.memory.recall_btn_search')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {trace && (
        <section aria-label={t('pane.memory.recall_results_aria')}>
          <p style={{ fontSize: '0.8rem', color: '#6b7280', margin: '0.75rem 0 0.5rem' }}>
            {t('pane.memory.recall_result_count', { count: trace.entries.length, total: trace.total })}
            {trace.session_id ? ` · session: ${trace.session_id}` : ''}
          </p>
          {trace.entries.length === 0 && (
            <div className="empty">{t('pane.memory.recall_empty')}</div>
          )}
          {trace.entries.length > 0 && (
            <table className="usage-table">
              <thead>
                <tr>
                  <th scope="col">{t('pane.memory.col_type')}</th>
                  <th scope="col">{t('pane.memory.col_preview')}</th>
                  <th scope="col">{t('pane.memory.col_tags')}</th>
                  <th scope="col">{t('pane.memory.recall_col_agent')}</th>
                  <th scope="col">{t('pane.memory.recall_col_score')}</th>
                  <th scope="col">{t('pane.memory.recall_col_recalled_at')}</th>
                </tr>
              </thead>
              <tbody>
                {trace.entries.map((entry: RecallEntry) => (
                  <tr key={`${entry.memory_id}-${entry.recalled_at}`}>
                    <td>
                      <span className={typeBadgeClass(entry.type)}>
                        {t(`pane.memory.type_${entry.type}`)}
                      </span>
                    </td>
                    <td style={{ maxWidth: '26rem', fontSize: '0.8rem', color: '#374151' }}>
                      {entry.content_preview}
                    </td>
                    <td style={{ fontSize: '0.78rem' }}>
                      {entry.tags.map((tag) => (
                        <span key={tag} className="kind-tag" style={{ marginRight: '0.25rem', opacity: 0.75 }}>
                          {tag}
                        </span>
                      ))}
                    </td>
                    <td style={{ fontSize: '0.8rem' }}>{entry.agent_id}</td>
                    <td style={{ fontSize: '0.8rem', fontVariantNumeric: 'tabular-nums' }}>
                      {(entry.relevance_score * 100).toFixed(1)}%
                    </td>
                    <td style={{ whiteSpace: 'nowrap', fontSize: '0.8rem' }}>
                      {fmtDate(entry.recalled_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Tab 3: Vector neighbors view
// ---------------------------------------------------------------------------

interface NeighborsViewProps {
  is404: boolean;
}

function NeighborsView({ is404 }: NeighborsViewProps): JSX.Element {
  const { t } = useTranslation();
  const [memoryId, setMemoryId] = useState('');
  const [topK, setTopK] = useState(5);
  const [anchor, setAnchor] = useState<MemoryRecord | null>(null);
  const [neighbors, setNeighbors] = useState<SimilarMemory[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSearch(): Promise<void> {
    if (!memoryId.trim()) return;
    setLoading(true);
    setError(null);
    try {
      if (is404) {
        const mockAnchor = MOCK_MEMORIES.find((m) => m.id === memoryId.trim()) ?? MOCK_MEMORIES[0]!;
        setAnchor(mockAnchor);
        setNeighbors(MOCK_SIMILAR);
      } else {
        const [anchorRec, simResp] = await Promise.all([
          client.getMemory(memoryId.trim()),
          client.findSimilarMemories(memoryId.trim(), { top_k: topK }),
        ]);
        setAnchor(anchorRec);
        setNeighbors(simResp.neighbors);
      }
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }

  return (
    <>
      <p style={{ fontSize: '0.85rem', color: '#6b7280', marginBottom: '0.75rem' }}>
        {t('pane.memory.neighbors_description')}
      </p>

      <div className="today-filters" role="group" aria-label={t('pane.memory.neighbors_filters_aria')}>
        <label>
          {t('pane.memory.neighbors_memory_id')}
          <input
            value={memoryId}
            onChange={(e) => setMemoryId(e.target.value)}
            className="search"
            placeholder="mem_abc123"
            style={{ marginLeft: '0.4rem', width: '16rem' }}
          />
        </label>
        <label>
          {t('pane.memory.neighbors_top_k')}
          <input
            type="number"
            min={1}
            max={20}
            value={topK}
            onChange={(e) => setTopK(Number(e.target.value))}
            style={{ marginLeft: '0.4rem', width: '4rem' }}
          />
        </label>
        <button onClick={() => void handleSearch()} disabled={loading || !memoryId.trim()}>
          {loading ? t('common.loading') : t('pane.memory.neighbors_btn_find')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {anchor && (
        <div
          style={{
            background: '#f0fdf4',
            border: '1px solid #86efac',
            borderRadius: '6px',
            padding: '0.75rem',
            margin: '0.75rem 0',
            fontSize: '0.85rem',
          }}
          aria-label={t('pane.memory.neighbors_anchor_aria')}
        >
          <strong>{t('pane.memory.neighbors_anchor_label')}</strong>{' '}
          <span className={typeBadgeClass(anchor.type)}>{t(`pane.memory.type_${anchor.type}`)}</span>{' '}
          <code style={{ fontSize: '0.78rem' }}>{anchor.id}</code>
          <p style={{ marginTop: '0.4rem', color: '#374151' }}>{preview(anchor.content)}</p>
        </div>
      )}

      {neighbors && (
        <section aria-label={t('pane.memory.neighbors_results_aria')}>
          {neighbors.length === 0 && (
            <div className="empty">{t('pane.memory.neighbors_empty')}</div>
          )}
          {neighbors.length > 0 && (
            <table className="usage-table">
              <thead>
                <tr>
                  <th scope="col">{t('pane.memory.col_type')}</th>
                  <th scope="col">{t('pane.memory.col_preview')}</th>
                  <th scope="col">{t('pane.memory.col_tags')}</th>
                  <th scope="col">{t('pane.memory.neighbors_col_similarity')}</th>
                  <th scope="col">{t('pane.memory.col_created')}</th>
                </tr>
              </thead>
              <tbody>
                {neighbors.map((nb: SimilarMemory) => (
                  <tr key={nb.memory_id}>
                    <td>
                      <span className={typeBadgeClass(nb.type)}>
                        {t(`pane.memory.type_${nb.type}`)}
                      </span>
                    </td>
                    <td style={{ maxWidth: '26rem', fontSize: '0.8rem', color: '#374151' }}>
                      {nb.content_preview}
                    </td>
                    <td style={{ fontSize: '0.78rem' }}>
                      {nb.tags.map((tag) => (
                        <span key={tag} className="kind-tag" style={{ marginRight: '0.25rem', opacity: 0.75 }}>
                          {tag}
                        </span>
                      ))}
                    </td>
                    <td
                      style={{
                        fontSize: '0.8rem',
                        fontVariantNumeric: 'tabular-nums',
                        color: nb.similarity > 0.85 ? '#dc2626' : nb.similarity > 0.7 ? '#d97706' : '#374151',
                        fontWeight: nb.similarity > 0.85 ? 600 : undefined,
                      }}
                    >
                      {(nb.similarity * 100).toFixed(1)}%
                    </td>
                    <td style={{ whiteSpace: 'nowrap', fontSize: '0.8rem' }}>
                      {fmtDate(nb.created_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function MemoryPane(): JSX.Element {
  const { t } = useTranslation();
  const [tab, setTab] = useState<TabId>('list');
  const [is404, setIs404] = useState(false);
  const [checked, setChecked] = useState(false);

  // Probe the memory endpoint once on mount to decide whether to use
  // the real API or fall back to mock data.
  useEffect(() => {
    let cancelled = false;
    client
      .listMemories({ limit: 1 })
      .then(() => {
        if (!cancelled) setIs404(false);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setIs404(e instanceof ApiError && e.status === 404);
        }
      })
      .finally(() => {
        if (!cancelled) setChecked(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!checked) {
    return (
      <>
        <header className="today-header">
          <h1>{t('pane.memory.title')}</h1>
        </header>
        <div className="empty">{t('common.loading')}</div>
      </>
    );
  }

  return (
    <>
      <header className="today-header">
        <h1>{t('pane.memory.title')}</h1>
        <div className="today-meta" style={{ fontSize: '0.8rem', color: '#6b7280' }}>
          {t('pane.memory.adr_label')}
        </div>
      </header>

      {is404 && <NotReadyBanner />}

      {/* T7.3 — import/export against the SHIPPED /v1/memories routes
          (works even while the tabs above still target the stale
          404-fallback-era /v1/memory contract). */}
      <MemoryImportExport />

      {/* Tab nav */}
      <div
        role="tablist"
        aria-label={t('pane.memory.tabs_aria')}
        style={{ display: 'flex', gap: '0.25rem', borderBottom: '1px solid var(--border, #e5e7eb)', marginBottom: '1rem' }}
      >
        {(
          [
            { id: 'list' as TabId, label: t('pane.memory.tab_list') },
            { id: 'recall' as TabId, label: t('pane.memory.tab_recall') },
            { id: 'neighbors' as TabId, label: t('pane.memory.tab_neighbors') },
          ] as const
        ).map(({ id, label }) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            onClick={() => setTab(id)}
            style={{
              padding: '0.4rem 0.9rem',
              border: 'none',
              borderBottom: tab === id ? '2px solid #3b82f6' : '2px solid transparent',
              background: 'none',
              cursor: 'pointer',
              fontWeight: tab === id ? 600 : 400,
              color: tab === id ? '#1d4ed8' : '#374151',
            }}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Tab panels */}
      {tab === 'list' && <ListView is404={is404} />}
      {tab === 'recall' && <RecallView is404={is404} />}
      {tab === 'neighbors' && <NeighborsView is404={is404} />}
    </>
  );
}
