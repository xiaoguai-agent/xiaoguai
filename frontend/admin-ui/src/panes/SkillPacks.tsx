/**
 * v1.3.x — Skill Pack Browser pane.
 *
 * Surfaces two APIs:
 *   GET  /v1/skills/installed  — list recorded packs
 *   POST /v1/skills/install    — record a new pack by ID
 *
 * Runtime loader activation is NOT yet wired server-side.  All installed
 * packs show an "Activation pending" badge and a prose notice is rendered
 * at the top of the page so operators are not misled.
 *
 * Layout:
 *   - Inline notice about activation-pending state
 *   - Install form (pack_id input + Install button + confirmation dialog)
 *   - Installed packs table with detail drawer
 *   - Empty / loading / error states
 */

import { useEffect, useRef, useState } from 'react';
import type { InstalledSkillPackResponse } from '@xiaoguai/shared';
import { client } from '../client';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type LoadState =
  | { kind: 'loading' }
  | { kind: 'ok'; packs: InstalledSkillPackResponse[] }
  | { kind: 'error'; message: string };

type InstallState =
  | { kind: 'idle' }
  | { kind: 'confirming'; packId: string }
  | { kind: 'installing' }
  | { kind: 'done'; name: string }
  | { kind: 'error'; message: string };

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function fmtDate(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

function commaSep(items: string[]): string {
  return items.length === 0 ? '—' : items.join(', ');
}

// ---------------------------------------------------------------------------
// Detail drawer
// ---------------------------------------------------------------------------

interface DetailDrawerProps {
  pack: InstalledSkillPackResponse;
  onClose: () => void;
}

function DetailDrawer({ pack, onClose }: DetailDrawerProps): JSX.Element {
  // Close on Escape key
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handler);
    closeRef.current?.focus();
    return () => document.removeEventListener('keydown', handler);
  }, [onClose]);

  return (
    <aside className="skill-drawer" role="complementary" aria-label="Skill pack details">
      <div className="skill-drawer-overlay" onClick={onClose} aria-hidden="true" />
      <div className="skill-drawer-panel">
        <header className="skill-drawer-header">
          <div>
            <h2 className="skill-drawer-title">{pack.name}</h2>
            <span className="skill-drawer-pack-id">{pack.pack_id}</span>
          </div>
          <button
            ref={closeRef}
            className="skill-drawer-close"
            onClick={onClose}
            aria-label="Close details"
          >
            ✕
          </button>
        </header>

        <div className="skill-drawer-body">
          <span className="skill-badge-pending">Activation pending</span>

          {pack.description && (
            <p className="skill-drawer-desc">{pack.description}</p>
          )}

          <dl className="skill-drawer-dl">
            <dt>Version</dt>
            <dd>
              <code>{pack.version}</code>
            </dd>

            <dt>Recorded at</dt>
            <dd>{fmtDate(pack.recorded_at)}</dd>

            <dt>Agents</dt>
            <dd>{commaSep(pack.agents)}</dd>

            <dt>Inbound adapters</dt>
            <dd>{commaSep(pack.inbound_adapters)}</dd>

            <dt>Outputs</dt>
            <dd>{commaSep(pack.outputs)}</dd>

            <dt>Activation status</dt>
            <dd>
              <span className="skill-badge-pending">pending</span>
              {' — '}
              runtime loader not yet wired; pack is recorded but inactive.
            </dd>
          </dl>
        </div>
      </div>
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Install form
// ---------------------------------------------------------------------------

interface InstallFormProps {
  state: InstallState;
  onSubmit: (packId: string) => void;
  onConfirm: () => void;
  onCancel: () => void;
}

function InstallForm({ state, onSubmit, onConfirm, onCancel }: InstallFormProps): JSX.Element {
  const [packId, setPackId] = useState('');

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = packId.trim();
    if (!trimmed) return;
    onSubmit(trimmed);
  }

  return (
    <section className="skill-install-section">
      <h2 className="skill-install-heading">Record a skill pack</h2>
      <form onSubmit={handleSubmit} className="skill-install-form">
        <input
          className="skill-install-input"
          type="text"
          placeholder="e.g. community/web-monitor@1.0.0"
          value={packId}
          onChange={(e) => setPackId(e.target.value)}
          disabled={state.kind === 'installing' || state.kind === 'confirming'}
          aria-label="Pack ID"
        />
        <button
          type="submit"
          className="skill-install-btn"
          disabled={
            !packId.trim() ||
            state.kind === 'installing' ||
            state.kind === 'confirming'
          }
        >
          {state.kind === 'installing' ? 'Recording…' : 'Install'}
        </button>
      </form>

      {/* Confirmation dialog */}
      {state.kind === 'confirming' && (
        <div className="skill-confirm" role="dialog" aria-modal="true" aria-label="Confirm install">
          <p>
            Record <strong>{state.packId}</strong>?{' '}
            <em>The pack will be marked "activation pending" — it will not run until the loader is wired.</em>
          </p>
          <div className="skill-confirm-actions">
            <button onClick={onConfirm} className="skill-install-btn">
              Confirm
            </button>
            <button onClick={onCancel} className="skill-cancel-btn">
              Cancel
            </button>
          </div>
        </div>
      )}

      {state.kind === 'done' && (
        <div className="skill-install-done">
          ✓ Recorded <strong>{state.name}</strong>{' '}
          <span className="skill-badge-pending">activation pending</span>
        </div>
      )}

      {state.kind === 'error' && (
        <div className="error skill-install-error">{state.message}</div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function SkillPacksPane(): JSX.Element {
  const [loadState, setLoadState] = useState<LoadState>({ kind: 'loading' });
  const [installState, setInstallState] = useState<InstallState>({ kind: 'idle' });
  const [selected, setSelected] = useState<InstalledSkillPackResponse | null>(null);
  // Remember packId across confirm/install cycle
  const pendingPackId = useRef<string>('');

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const packs = await client.listInstalledSkillPacks();
        if (!cancelled) setLoadState({ kind: 'ok', packs });
      } catch (err) {
        if (!cancelled) setLoadState({ kind: 'error', message: (err as Error).message });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  function handleInstallSubmit(packId: string) {
    pendingPackId.current = packId;
    setInstallState({ kind: 'confirming', packId });
  }

  async function handleInstallConfirm() {
    const packId = pendingPackId.current;
    setInstallState({ kind: 'installing' });
    try {
      const resp = await client.installSkillPack({ pack_id: packId });
      setInstallState({ kind: 'done', name: resp.name });
      // Reload the installed list
      try {
        const packs = await client.listInstalledSkillPacks();
        setLoadState({ kind: 'ok', packs });
      } catch {
        // best-effort; don't clobber the success message
      }
    } catch (err) {
      setInstallState({ kind: 'error', message: (err as Error).message });
    }
  }

  function handleInstallCancel() {
    setInstallState({ kind: 'idle' });
  }

  const packs = loadState.kind === 'ok' ? loadState.packs : [];

  return (
    <>
      <h1>Skill Packs</h1>

      {/* Activation-pending notice — always visible */}
      <div className="skill-activation-notice" role="note">
        <strong>Activation pending for all packs:</strong> The runtime skill loader
        is not yet wired. Recording a pack registers it in the database; it will
        not run until a future release activates the loader.
      </div>

      <InstallForm
        state={installState}
        onSubmit={handleInstallSubmit}
        onConfirm={() => void handleInstallConfirm()}
        onCancel={handleInstallCancel}
      />

      <section aria-label="Installed skill packs">
        <h2 className="skill-section-heading">
          Installed packs
          {loadState.kind === 'ok' && (
            <span className="skill-count">({packs.length})</span>
          )}
        </h2>

        {loadState.kind === 'loading' && (
          <div className="empty">Loading…</div>
        )}

        {loadState.kind === 'error' && (
          <div className="error">Failed to load installed packs: {loadState.message}</div>
        )}

        {loadState.kind === 'ok' && packs.length === 0 && (
          <div className="empty">
            No skill packs recorded yet. Use the form above to record a pack by ID.
          </div>
        )}

        {loadState.kind === 'ok' && packs.length > 0 && (
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Pack ID</th>
                <th>Version</th>
                <th>Agents</th>
                <th>Status</th>
                <th>Recorded at</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {packs.map((pack) => (
                <tr key={pack.id}>
                  <td>{pack.name}</td>
                  <td>
                    <code>{pack.pack_id}</code>
                  </td>
                  <td>
                    <span className="tag">{pack.version}</span>
                  </td>
                  <td>
                    {pack.agents.length === 0 ? (
                      <em style={{ color: 'var(--muted)', fontSize: '12px' }}>none</em>
                    ) : (
                      pack.agents.join(', ')
                    )}
                  </td>
                  <td>
                    <span className="skill-badge-pending">Activation pending</span>
                  </td>
                  <td style={{ fontSize: '12px', color: 'var(--muted)' }}>
                    {fmtDate(pack.recorded_at)}
                  </td>
                  <td>
                    <button
                      className="skill-detail-btn"
                      onClick={() => setSelected(pack)}
                      aria-label={`View details for ${pack.name}`}
                    >
                      Details
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      {selected !== null && (
        <DetailDrawer pack={selected} onClose={() => setSelected(null)} />
      )}
    </>
  );
}
