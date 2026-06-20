/**
 * T7.3 — memory export / import toolbar for the Memory pane.
 *
 * Talks to the SHIPPED T7.2 routes (`GET /v1/memories/export`,
 * `POST /v1/memories/import` — JSONL over text/plain), unlike the rest of
 * the Memory pane which still targets the planned-but-never-shipped
 * `/v1/memory/*` contract (404-fallback era).
 *
 * Export: downloads the whole store as `memories.jsonl` via a synthesised
 * anchor (same precedent as the Audit pane export).
 * Import: file picker → line-count preview → confirm → inline
 * `{imported, skipped}` report including per-line skip reasons.
 */

import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { MemoryImportReport, XiaoguaiClient } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { ErrorBanner } from '../components/ErrorBanner';

export const EXPORT_FILENAME = 'memories.jsonl';

/** Count non-blank lines — the preview shown before a confirmed import. */
export function countJsonlLines(text: string): number {
  return text.split('\n').filter((line) => line.trim().length > 0).length;
}

type ImportState =
  | { kind: 'idle' }
  | { kind: 'preview'; text: string; lineCount: number; filename: string }
  | { kind: 'importing' }
  | { kind: 'done'; report: MemoryImportReport };

export interface MemoryImportExportProps {
  /** Override the shared client (used by tests). */
  client?: Pick<XiaoguaiClient, 'exportMemories' | 'importMemories'>;
  /** Invoked after a successful import so the pane can refresh its list. */
  onImported?: (report: MemoryImportReport) => void;
}

export function MemoryImportExport({
  client,
  onImported,
}: MemoryImportExportProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [imp, setImp] = useState<ImportState>({ kind: 'idle' });

  async function onExport(): Promise<void> {
    if (exporting) return;
    setExporting(true);
    setError(null);
    try {
      const text = await c.exportMemories();
      const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
      const url = URL.createObjectURL(blob);
      try {
        const a = document.createElement('a');
        a.href = url;
        a.download = EXPORT_FILENAME;
        document.body.appendChild(a);
        a.click();
        a.remove();
      } finally {
        URL.revokeObjectURL(url);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setExporting(false);
    }
  }

  async function onFileChosen(file: File): Promise<void> {
    setError(null);
    try {
      const text = await file.text();
      setImp({
        kind: 'preview',
        text,
        lineCount: countJsonlLines(text),
        filename: file.name,
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function onConfirmImport(): Promise<void> {
    if (imp.kind !== 'preview') return;
    const { text } = imp;
    setImp({ kind: 'importing' });
    setError(null);
    try {
      const report = await c.importMemories(text);
      setImp({ kind: 'done', report });
      onImported?.(report);
    } catch (err) {
      setImp({ kind: 'idle' });
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  function resetImport(): void {
    setImp({ kind: 'idle' });
    if (fileInputRef.current) fileInputRef.current.value = '';
  }

  return (
    <div
      role="group"
      aria-label={t('pane.memory.import_export_aria')}
      style={{ margin: '0.5rem 0 1rem' }}
    >
      <div style={{ display: 'flex', gap: '0.5rem', alignItems: 'center', flexWrap: 'wrap' }}>
        <button
          type="button"
          data-testid="memory-export-btn"
          onClick={() => void onExport()}
          disabled={exporting}
        >
          {exporting ? t('common.loading') : t('pane.memory.btn_export')}
        </button>
        <button
          type="button"
          data-testid="memory-import-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={imp.kind === 'importing'}
        >
          {t('pane.memory.btn_import')}
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept=".jsonl,.txt,text/plain"
          aria-label={t('pane.memory.import_file_aria')}
          style={{ display: 'none' }}
          onChange={(e) => {
            const file = e.target.files?.[0];
            if (file) void onFileChosen(file);
          }}
        />
      </div>

      <ErrorBanner message={error} />

      {imp.kind === 'preview' && (
        <div
          role="status"
          style={{
            marginTop: '0.5rem',
            padding: '0.5rem 0.75rem',
            border: '1px solid var(--border, #e5e7eb)',
            borderRadius: '6px',
            fontSize: '0.85rem',
          }}
        >
          <span>
            {t('pane.memory.import_preview', {
              filename: imp.filename,
              count: imp.lineCount,
            })}
          </span>{' '}
          <button type="button" onClick={() => void onConfirmImport()}>
            {t('pane.memory.btn_import_confirm')}
          </button>{' '}
          <button type="button" onClick={resetImport}>
            {t('pane.memory.btn_import_cancel')}
          </button>
        </div>
      )}

      {imp.kind === 'importing' && <p role="status">{t('common.loading')}</p>}

      {imp.kind === 'done' && (
        <div
          role="status"
          data-testid="memory-import-result"
          style={{
            marginTop: '0.5rem',
            padding: '0.5rem 0.75rem',
            border: '1px solid var(--border, #e5e7eb)',
            borderRadius: '6px',
            fontSize: '0.85rem',
          }}
        >
          <p style={{ margin: 0 }}>
            {t('pane.memory.import_result', {
              imported: imp.report.imported,
              skipped: imp.report.skipped.length,
            })}
          </p>
          {imp.report.skipped.length > 0 && (
            <ul style={{ margin: '0.4rem 0 0', paddingLeft: '1.25rem' }}>
              {imp.report.skipped.map((s) => (
                <li key={s.line} style={{ color: '#92400e' }}>
                  {t('pane.memory.import_skipped_line', {
                    line: s.line,
                    reason: s.reason,
                  })}
                </li>
              ))}
            </ul>
          )}
          {/* #288: early abort (e.g. embedder outage) — remaining lines
              were never attempted, so make the reason impossible to miss. */}
          {imp.report.aborted && (
            <p className="error" data-testid="memory-import-aborted" style={{ marginTop: '0.4rem' }}>
              {t('common.failed', { message: imp.report.aborted })}
            </p>
          )}{' '}
          <button type="button" onClick={resetImport}>
            {t('common.close')}
          </button>
        </div>
      )}
    </div>
  );
}
