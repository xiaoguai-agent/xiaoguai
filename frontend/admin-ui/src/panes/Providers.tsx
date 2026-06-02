import { useCallback, useEffect, useState } from 'react';
import type { FormEvent } from 'react';
import type { CreateProviderRequest, LlmProviderView, ProviderKind } from '@xiaoguai/shared';
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

/**
 * Providers pane — register an LLM provider pointing at a local model URL
 * (Ollama / any OpenAI-compatible server) or a hosted API (MiniMax, Zhipu,
 * OpenAI/codex, DeepSeek, …). The API key is stored server-side; new providers
 * take effect after the server restarts (the router is built at boot).
 */
export function ProvidersPane() {
  const [rows, setRows] = useState<LlmProviderView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [name, setName] = useState('');
  const [kind, setKind] = useState<ProviderKind>('openai_compat');
  const [endpoint, setEndpoint] = useState('');
  const [models, setModels] = useState('');
  const [apiKey, setApiKey] = useState('');

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
        models: models
          .split(',')
          .map((s) => s.trim())
          .filter(Boolean),
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

  const remove = async (id: string) => {
    if (!window.confirm('Delete this provider?')) return;
    setError(null);
    try {
      await client.deleteProvider(id);
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <>
      <h1>LLM Providers</h1>
      <p className="hint">
        Point at a local model URL (Ollama / OpenAI-compatible server) or a hosted API
        (MiniMax, Zhipu, OpenAI, DeepSeek…). New providers take effect after the server restarts.
      </p>
      {error && <div className="error">{error}</div>}

      <form onSubmit={submit} className="provider-form">
        <input
          placeholder="Name (e.g. Local Ollama)"
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
          placeholder="Endpoint URL (http://localhost:11434 or https://api.minimax.io)"
          value={endpoint}
          onChange={(e) => setEndpoint(e.target.value)}
          required
        />
        <input
          placeholder="Models (comma-separated, e.g. qwen2.5-coder,MiniMax-M2)"
          value={models}
          onChange={(e) => setModels(e.target.value)}
        />
        <input
          type="password"
          placeholder="API key (leave blank for local URLs)"
          value={apiKey}
          onChange={(e) => setApiKey(e.target.value)}
        />
        <button type="submit" disabled={busy}>
          {busy ? 'Adding…' : 'Add provider'}
        </button>
      </form>

      {rows === null ? (
        <p>Loading…</p>
      ) : rows.length === 0 ? (
        <p className="empty">No providers yet. Add one above.</p>
      ) : (
        <table className="provider-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Kind</th>
              <th>Endpoint</th>
              <th>Models</th>
              <th>Key</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => (
              <tr key={p.id}>
                <td>{p.name}</td>
                <td>{p.kind}</td>
                <td>{p.endpoint}</td>
                <td>{p.models.join(', ')}</td>
                <td>{p.has_api_key ? '✓ stored' : p.api_key_env ? `env:${p.api_key_env}` : '—'}</td>
                <td>
                  <button type="button" className="danger" onClick={() => void remove(p.id)}>
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}
