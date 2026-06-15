/**
 * Memory browser / editor — wired to the SHIPPED `/v1/memories` routes
 * (see `crates/xiaoguai-api/src/routes/memory.rs`). The backend returns
 * 503 while `memory_store` is unconfigured; that surfaces as a plain
 * error banner here.
 *
 * Three tabs:
 *   1. List      — filterable table by kind / tag
 *   2. Recall    — semantic recall test: query → ranked {memory, score}
 *   3. Neighbors — vector-similarity neighborhood for a given memory
 */

import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  MemoryKind,
  MemoryRecord,
  RecalledMemory,
} from '@xiaoguai/shared';
import { MEMORY_KINDS } from '@xiaoguai/shared';
import { client } from '../client';
import { PaneIntro } from '../components/PaneIntro';
import { MemoryImportExport } from './MemoryImportExport';

type TabId = 'list' | 'recall' | 'neighbors';

// ---------------------------------------------------------------------------
// Helpers (exported for unit tests)
// ---------------------------------------------------------------------------

export function fmtDate(iso: string | null): string {
  if (!iso) return '—';
  return new Date(iso).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function preview(content: string): string {
  const lines = content.split('\n').slice(0, 2).join(' ');
  return lines.length > 120 ? `${lines.slice(0, 120)}…` : lines;
}

export function kindBadgeClass(kind: MemoryKind): string {
  if (kind === 'facts') return 'kind-tag kind-tag-chat';
  if (kind === 'episodes') return 'kind-tag kind-tag-scheduled';
  return 'kind-tag kind-tag-im';
}

/** i18n key for a kind label (reuses the existing type_* keys). */
export function kindLabelKey(kind: MemoryKind): string {
  if (kind === 'facts') return 'pane.memory.type_fact';
  if (kind === 'episodes') return 'pane.memory.type_episode';
  return 'pane.memory.type_preference';
}

/** Comma-separated input → trimmed, non-empty tag list. */
export function tagsFromRaw(raw: string): string[] {
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
}

/** `datetime-local` input value → RFC 3339 (UTC), or null when blank. */
export function localInputToIso(value: string): string | null {
  const v = value.trim();
  if (!v) return null;
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

/** RFC 3339 → `datetime-local` input value (local time), '' when null. */
export function isoToLocalInput(iso: string | null): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n: number): string => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

// ---------------------------------------------------------------------------
// New / Edit memory modal
// ---------------------------------------------------------------------------

interface MemoryFormProps {
  existing: MemoryRecord | null;
  onClose: () => void;
  onSaved: (record: MemoryRecord) => void;
}

function MemoryModal({ existing, onClose, onSaved }: MemoryFormProps): JSX.Element {
  const { t } = useTranslation();
  const isNew = existing === null;

  const [kind, setKind] = useState<MemoryKind>(existing?.kind ?? 'facts');
  const [content, setContent] = useState(existing?.content ?? '');
  const [tagsRaw, setTagsRaw] = useState((existing?.tags ?? []).join(', '));
  const [ttlLocal, setTtlLocal] = useState(isoToLocalInput(existing?.ttl_at ?? null));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSave(): Promise<void> {
    setSaving(true);
    setError(null);
    const tags = tagsFromRaw(tagsRaw);
    const ttlAt = localInputToIso(ttlLocal);

    try {
      let saved: MemoryRecord;
      if (isNew) {
        saved = await client.createMemory({ kind, content, tags, ttl_at: ttlAt });
      } else {
        saved = await client.updateMemory(existing!.id, { content, tags, ttl_at: ttlAt });
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
              value={kind}
              onChange={(e) => setKind(e.target.value as MemoryKind)}
              disabled={!isNew}
              style={{ marginLeft: '0.5rem' }}
            >
              {MEMORY_KINDS.map((mk) => (
                <option key={mk} value={mk}>
                  {t(kindLabelKey(mk))}
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
              type="datetime-local"
              value={ttlLocal}
              onChange={(e) => setTtlLocal(e.target.value)}
              style={{ marginLeft: '0.5rem' }}
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
}

function DeleteConfirmModal({ record, onCancel, onConfirmed }: DeleteConfirmProps): JSX.Element {
  const { t } = useTranslation();
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleDelete(): Promise<void> {
    setDeleting(true);
    setError(null);
    try {
      await client.deleteMemory(record.id);
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
  /**
   * #288: bumped by the pane after a successful JSONL import so the list
   * reloads (the import toolbar lives outside this tab's state).
   */
  refreshToken?: number;
}

function ListView({ refreshToken = 0 }: ListViewProps): JSX.Element {
  const { t } = useTranslation();

  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Filters
  const [filterKind, setFilterKind] = useState<MemoryKind | ''>('');
  const [filterTag, setFilterTag] = useState('');

  // Modals: undefined = closed, null = new, MemoryRecord = edit existing
  const [editingRecord, setEditingRecord] = useState<MemoryRecord | null | undefined>(undefined);
  const [deletingRecord, setDeletingRecord] = useState<MemoryRecord | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const tag = filterTag.trim();
      const resp = await client.listMemories({
        kind: filterKind || undefined,
        tags: tag ? [tag] : undefined,
        limit: 50,
      });
      setRecords(resp);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [filterKind, filterTag]);

  useEffect(() => {
    void load();
    // #288: re-run when refreshToken bumps (post-import refresh).
  }, [load, refreshToken]);

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
    setDeletingRecord(null);
  }

  return (
    <>
      {/* Filters */}
      <div className="today-filters" role="group" aria-label={t('pane.memory.filters_aria')}>
        <label>
          {t('pane.memory.filter_type')}
          <select
            value={filterKind}
            onChange={(e) => setFilterKind(e.target.value as MemoryKind | '')}
            style={{ marginLeft: '0.4rem' }}
          >
            <option value="">{t('pane.memory.type_all')}</option>
            {MEMORY_KINDS.map((mk) => (
              <option key={mk} value={mk}>
                {t(kindLabelKey(mk))}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t('pane.memory.filter_tag')}
          <input
            value={filterTag}
            onChange={(e) => setFilterTag(e.target.value)}
            className="search"
            placeholder="tag"
            style={{ marginLeft: '0.4rem', width: '9rem' }}
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
        {t('pane.memory.list_count', { count: records.length, total: records.length })}
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
                  <span className={kindBadgeClass(rec.kind)}>{t(kindLabelKey(rec.kind))}</span>
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
          existing={editingRecord}
          onClose={() => setEditingRecord(undefined)}
          onSaved={handleSaved}
        />
      )}

      {deletingRecord && (
        <DeleteConfirmModal
          record={deletingRecord}
          onCancel={() => setDeletingRecord(null)}
          onConfirmed={() => handleDeleted(deletingRecord.id)}
        />
      )}
    </>
  );
}

// ---------------------------------------------------------------------------
// Tab 2: Semantic recall view
// ---------------------------------------------------------------------------

function RecallView(): JSX.Element {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [topK, setTopK] = useState(5);
  const [kindFilter, setKindFilter] = useState<MemoryKind | ''>('');
  const [hits, setHits] = useState<RecalledMemory[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSearch(): Promise<void> {
    if (!query.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const resp = await client.recallMemories({
        query: query.trim(),
        top_k: topK,
        kind_filter: kindFilter || undefined,
      });
      setHits(resp);
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
          {t('pane.memory.recall_query')}
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="search"
            placeholder={t('pane.memory.recall_query_placeholder')}
            style={{ marginLeft: '0.4rem', width: '18rem' }}
          />
        </label>
        <label>
          {t('pane.memory.filter_type')}
          <select
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value as MemoryKind | '')}
            style={{ marginLeft: '0.4rem' }}
          >
            <option value="">{t('pane.memory.type_all')}</option>
            {MEMORY_KINDS.map((mk) => (
              <option key={mk} value={mk}>
                {t(kindLabelKey(mk))}
              </option>
            ))}
          </select>
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
        <button onClick={() => void handleSearch()} disabled={loading || !query.trim()}>
          {loading ? t('common.loading') : t('pane.memory.recall_btn_search')}
        </button>
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {hits && (
        <section aria-label={t('pane.memory.recall_results_aria')}>
          <p style={{ fontSize: '0.8rem', color: '#6b7280', margin: '0.75rem 0 0.5rem' }}>
            {t('pane.memory.recall_result_count', { count: hits.length, total: hits.length })}
          </p>
          {hits.length === 0 && <div className="empty">{t('pane.memory.recall_empty')}</div>}
          {hits.length > 0 && (
            <table className="usage-table">
              <thead>
                <tr>
                  <th scope="col">{t('pane.memory.col_type')}</th>
                  <th scope="col">{t('pane.memory.col_preview')}</th>
                  <th scope="col">{t('pane.memory.col_tags')}</th>
                  <th scope="col">{t('pane.memory.recall_col_score')}</th>
                  <th scope="col">{t('pane.memory.col_created')}</th>
                </tr>
              </thead>
              <tbody>
                {hits.map((hit) => (
                  <tr key={hit.memory.id}>
                    <td>
                      <span className={kindBadgeClass(hit.memory.kind)}>
                        {t(kindLabelKey(hit.memory.kind))}
                      </span>
                    </td>
                    <td style={{ maxWidth: '26rem', fontSize: '0.8rem', color: '#374151' }}>
                      {preview(hit.memory.content)}
                    </td>
                    <td style={{ fontSize: '0.78rem' }}>
                      {hit.memory.tags.map((tag) => (
                        <span key={tag} className="kind-tag" style={{ marginRight: '0.25rem', opacity: 0.75 }}>
                          {tag}
                        </span>
                      ))}
                    </td>
                    <td style={{ fontSize: '0.8rem', fontVariantNumeric: 'tabular-nums' }}>
                      {(hit.score * 100).toFixed(1)}%
                    </td>
                    <td style={{ whiteSpace: 'nowrap', fontSize: '0.8rem' }}>
                      {fmtDate(hit.memory.created_at)}
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

function NeighborsView(): JSX.Element {
  const { t } = useTranslation();
  const [memoryId, setMemoryId] = useState('');
  const [topK, setTopK] = useState(5);
  const [anchor, setAnchor] = useState<MemoryRecord | null>(null);
  const [neighbors, setNeighbors] = useState<RecalledMemory[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSearch(): Promise<void> {
    if (!memoryId.trim()) return;
    setLoading(true);
    setError(null);
    try {
      const [anchorRec, similar] = await Promise.all([
        client.getMemory(memoryId.trim()),
        client.findSimilarMemories(memoryId.trim(), { top_k: topK }),
      ]);
      setAnchor(anchorRec);
      setNeighbors(similar);
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
          <span className={kindBadgeClass(anchor.kind)}>{t(kindLabelKey(anchor.kind))}</span>{' '}
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
                {neighbors.map((nb) => (
                  <tr key={nb.memory.id}>
                    <td>
                      <span className={kindBadgeClass(nb.memory.kind)}>
                        {t(kindLabelKey(nb.memory.kind))}
                      </span>
                    </td>
                    <td style={{ maxWidth: '26rem', fontSize: '0.8rem', color: '#374151' }}>
                      {preview(nb.memory.content)}
                    </td>
                    <td style={{ fontSize: '0.78rem' }}>
                      {nb.memory.tags.map((tag) => (
                        <span key={tag} className="kind-tag" style={{ marginRight: '0.25rem', opacity: 0.75 }}>
                          {tag}
                        </span>
                      ))}
                    </td>
                    <td
                      style={{
                        fontSize: '0.8rem',
                        fontVariantNumeric: 'tabular-nums',
                        color: nb.score > 0.85 ? '#dc2626' : nb.score > 0.7 ? '#d97706' : '#374151',
                        fontWeight: nb.score > 0.85 ? 600 : undefined,
                      }}
                    >
                      {(nb.score * 100).toFixed(1)}%
                    </td>
                    <td style={{ whiteSpace: 'nowrap', fontSize: '0.8rem' }}>
                      {fmtDate(nb.memory.created_at)}
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
  // #288: bump after a successful import so the List tab reloads.
  const [refreshToken, setRefreshToken] = useState(0);

  return (
    <>
      <header className="today-header">
        <h1>{t('pane.memory.title')}</h1>
      </header>

      <PaneIntro
        purpose={t('pane.memory.intro.purpose')}
        usage={t('pane.memory.intro.usage')}
        usageLabel={t('pane.memory.intro.usage_label')}
      />

      {/* T7.3 — JSONL import/export toolbar (shared /v1/memories routes).
          #288: onImported wires the post-import list refresh. */}
      <MemoryImportExport onImported={() => setRefreshToken((n) => n + 1)} />

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
      {tab === 'list' && <ListView refreshToken={refreshToken} />}
      {tab === 'recall' && <RecallView />}
      {tab === 'neighbors' && <NeighborsView />}
    </>
  );
}
