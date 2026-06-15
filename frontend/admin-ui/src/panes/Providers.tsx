import { useCallback, useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import { useTranslation } from 'react-i18next';
import type {
  CreateProviderRequest,
  LlmProviderView,
  ProviderKind,
  UpdateProviderRequest,
} from '@xiaoguai/shared';
import { client } from '../client';

/** Selectable provider kinds (local-URL kinds first). */
const KINDS: ProviderKind[] = [
  'ollama',
  'openai_compat',
  'minimax',
  'anthropic',
  'gemini',
  'mistral',
  'groq',
  'azure_openai',
  'bedrock',
];

/** Endpoint presets surfaced as `<datalist>` suggestions on both the add and
 *  edit forms — one click picks the MiniMax China domestic domain, the MiniMax
 *  international domain, or a local Ollama URL. */
const ENDPOINT_PRESETS: { value: string; labelKey: string }[] = [
  { value: 'https://api.minimax.io', labelKey: 'pane.providers.preset_minimax_intl' },
  { value: 'https://api.minimaxi.com', labelKey: 'pane.providers.preset_minimax_cn' },
  { value: 'http://localhost:11434', labelKey: 'pane.providers.preset_ollama' },
];

/** Shared id for the endpoint suggestion list (referenced by both forms). */
const ENDPOINT_LIST_ID = 'provider-endpoint-presets';

/** Per-row edit form state. `apiKey` starts blank — an empty value keeps the
 *  stored secret (the server never returns it). */
interface EditDraft {
  name: string;
  endpoint: string;
  models: string;
  fallbackOrder: string;
  apiKey: string;
}

/** Seed an edit draft from a provider row (API key intentionally blank). */
function draftFrom(p: LlmProviderView): EditDraft {
  return {
    name: p.name,
    endpoint: p.endpoint,
    models: p.models.join(', '),
    fallbackOrder: String(p.fallback_order),
    apiKey: '',
  };
}

/** Split a comma-separated model list into a trimmed, de-blanked array. */
function parseModels(raw: string): string[] {
  return raw
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean);
}

/**
 * Providers pane — register an LLM provider pointing at a local model URL
 * (Ollama / any OpenAI-compatible server) or a hosted API (MiniMax, Zhipu,
 * OpenAI/codex, DeepSeek, …). The API key is stored server-side; new providers
 * and edits take effect after the server restarts (the router is built at boot).
 *
 * Each row can be edited in place (the "Edit" button) — this is how a seeded
 * MiniMax provider gets its API key pasted in, or its endpoint switched to the
 * China domestic domain, without dropping to the CLI.
 */
export function ProvidersPane() {
  const { t } = useTranslation();
  const [rows, setRows] = useState<LlmProviderView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [name, setName] = useState('');
  const [kind, setKind] = useState<ProviderKind>('openai_compat');
  const [endpoint, setEndpoint] = useState('');
  const [models, setModels] = useState('');
  const [apiKey, setApiKey] = useState('');

  // Inline edit: which row is open + its working draft.
  const [editId, setEditId] = useState<string | null>(null);
  const [draft, setDraft] = useState<EditDraft | null>(null);
  const [editBusy, setEditBusy] = useState(false);

  const load = useCallback(async () => {
    setError(null);
    try {
      setRows(await client.listProviders());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const req: CreateProviderRequest = {
        name,
        kind,
        endpoint,
        models: parseModels(models),
        ...(apiKey.trim() ? { api_key: apiKey.trim() } : {}),
      };
      await client.createProvider(req);
      setName('');
      setEndpoint('');
      setModels('');
      setApiKey('');
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const startEdit = (p: LlmProviderView) => {
    setError(null);
    setEditId(p.id);
    setDraft(draftFrom(p));
  };

  const cancelEdit = () => {
    setEditId(null);
    setDraft(null);
  };

  const saveEdit = async (e: FormEvent, original: LlmProviderView) => {
    e.preventDefault();
    if (!draft) return;
    setEditBusy(true);
    setError(null);
    try {
      // Send only the fields the user actually changed. The API key is sent
      // only when non-empty (an empty value keeps the stored secret).
      const req: UpdateProviderRequest = {};
      const trimmedName = draft.name.trim();
      if (trimmedName && trimmedName !== original.name) req.name = trimmedName;

      const trimmedEndpoint = draft.endpoint.trim();
      if (trimmedEndpoint !== original.endpoint) req.endpoint = trimmedEndpoint;

      const nextModels = parseModels(draft.models);
      if (nextModels.join(',') !== original.models.join(',')) req.models = nextModels;

      const nextOrder = Number.parseInt(draft.fallbackOrder, 10);
      if (Number.isFinite(nextOrder) && nextOrder !== original.fallback_order) {
        req.fallback_order = nextOrder;
      }

      if (draft.apiKey.trim()) req.api_key = draft.apiKey.trim();

      await client.updateProvider(original.id, req);
      cancelEdit();
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setEditBusy(false);
    }
  };

  const remove = async (id: string) => {
    if (!window.confirm(t('pane.providers.confirm_delete'))) return;
    setError(null);
    if (editId === id) cancelEdit();
    try {
      await client.deleteProvider(id);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <>
      <h1>{t('pane.providers.title')}</h1>
      <p className="hint">{t('pane.providers.hint')}</p>
      <p className="hint">{t('pane.providers.restart_hint')}</p>
      {error && <div className="error">{error}</div>}

      {/* Shared endpoint suggestions for both the add and edit forms. */}
      <datalist id={ENDPOINT_LIST_ID}>
        {ENDPOINT_PRESETS.map((preset) => (
          <option key={preset.value} value={preset.value}>
            {t(preset.labelKey)}
          </option>
        ))}
      </datalist>

      <form onSubmit={submit} className="provider-form">
        <input
          placeholder={t('pane.providers.name_placeholder')}
          value={name}
          onChange={(e) => setName(e.target.value)}
          required
        />
        <select value={kind} onChange={(e) => setKind(e.target.value as ProviderKind)}>
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {k}
            </option>
          ))}
        </select>
        <input
          placeholder={t('pane.providers.endpoint_placeholder')}
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
          list={ENDPOINT_LIST_ID}
          required
        />
        <input
          placeholder={t('pane.providers.models_placeholder')}
          value={models}
          onChange={(e) => setModels(e.target.value)}
        />
        <input
          type="password"
          placeholder={t('pane.providers.api_key_placeholder')}
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
        />
        <button type="submit" disabled={busy}>
          {busy ? t('pane.providers.adding') : t('pane.providers.add')}
        </button>
      </form>

      {rows === null ? (
        <p>{t('common.loading')}</p>
      ) : rows.length === 0 ? (
        <p className="empty">{t('pane.providers.empty')}</p>
      ) : (
        <table className="provider-table">
          <thead>
            <tr>
              <th>{t('pane.providers.col_name')}</th>
              <th>{t('pane.providers.col_kind')}</th>
              <th>{t('pane.providers.col_endpoint')}</th>
              <th>{t('pane.providers.col_models')}</th>
              <th>{t('pane.providers.col_key')}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => {
              const isEditing = editId === p.id && draft !== null;
              return isEditing ? (
                <tr key={p.id} className="provider-edit-row">
                  <td colSpan={6}>
                    <form
                      className="provider-edit-form"
                      onSubmit={(e) => void saveEdit(e, p)}
                    >
                      <label>
                        {t('pane.providers.col_name')}
                        <input
                          value={draft.name}
                          onChange={(e) =>
                            setDraft({ ...draft, name: e.target.value })
                          }
                          required
                        />
                      </label>
                      <label>
                        {t('pane.providers.col_endpoint')}
                        <input
                          value={draft.endpoint}
                          onChange={(e) =>
                            setDraft({ ...draft, endpoint: e.target.value })
                          }
                          list={ENDPOINT_LIST_ID}
                        />
                      </label>
                      <label>
                        {t('pane.providers.col_models')}
                        <input
                          placeholder={t('pane.providers.models_placeholder')}
                          value={draft.models}
                          onChange={(e) =>
                            setDraft({ ...draft, models: e.target.value })
                          }
                        />
                      </label>
                      <label>
                        {t('pane.providers.fallback_order_label')}
                        <input
                          type="number"
                          value={draft.fallbackOrder}
                          onChange={(e) =>
                            setDraft({ ...draft, fallbackOrder: e.target.value })
                          }
                        />
                      </label>
                      <label>
                        {t('pane.providers.col_key')}
                        <input
                          type="password"
                          placeholder={t('pane.providers.api_key_keep_placeholder')}
                          value={draft.apiKey}
                          onChange={(e) =>
                            setDraft({ ...draft, apiKey: e.target.value })
                          }
                        />
                      </label>
                      <div className="provider-edit-actions">
                        <button type="submit" disabled={editBusy}>
                          {editBusy
                            ? t('pane.providers.saving')
                            : t('pane.providers.save')}
                        </button>
                        <button
                          type="button"
                          onClick={cancelEdit}
                          disabled={editBusy}
                        >
                          {t('common.close')}
                        </button>
                      </div>
                    </form>
                  </td>
                </tr>
              ) : (
                <tr key={p.id}>
                  <td>{p.name}</td>
                  <td>{p.kind}</td>
                  <td>{p.endpoint}</td>
                  <td>{p.models.join(', ')}</td>
                  <td>
                    {p.has_api_key
                      ? t('pane.providers.key_stored')
                      : p.api_key_env
                        ? `env:${p.api_key_env}`
                        : '—'}
                  </td>
                  <td className="provider-row-actions">
                    <button type="button" onClick={() => startEdit(p)}>
                      {t('pane.providers.edit')}
                    </button>
                    <button
                      type="button"
                      className="danger"
                      onClick={() => void remove(p.id)}
                    >
                      {t('pane.providers.delete')}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </>
  );
}
