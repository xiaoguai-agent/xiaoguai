/**
 * v1.4 — Anomaly Detector Dashboard.
 *
 * NOTE: The /v1/anomaly/* REST endpoints are PLANNED but not yet
 * implemented in xiaoguai-api (the xiaoguai-anomaly crate is currently a
 * pure Rust library with no HTTP surface). The UI renders a placeholder
 * banner when endpoints return 404 or 503, and degrades gracefully
 * otherwise. All four client methods handle these errors explicitly.
 *
 * Layout:
 *   1. Recent detections list — filterable by detector / severity / range.
 *      Each row: detector_id, fired_at, severity, series_key, value,
 *      threshold, "False positive?" thumbs-down button.
 *   2. Per-detector tuning panel — sigma_threshold, alpha (EWMA only),
 *      window_secs, min_count, cool_off_secs sliders + "Apply tuning"
 *      button (HotL-gated: requires window.confirm before submit).
 *   3. 14-day trend sparkline tiles — one tile per detector.
 */

import { useCallback, useEffect, useState } from 'react';
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import type {
  AnomalyDetection,
  AnomalyDetectorConfig,
  AnomalyDetectorKind,
  AnomalyDetectorPatch,
  AnomalyFireRateBucket,
  AnomalySeverity,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { useTranslation } from 'react-i18next';
import { client } from '../client';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SEVERITY_OPTIONS: Array<AnomalySeverity | ''> = ['', 'low', 'medium', 'high'];

const SEVERITY_COLOR: Record<AnomalySeverity, string> = {
  low: '#f59e0b',
  medium: '#ef4444',
  high: '#7c3aed',
};

const TIME_RANGES = [
  { label: 'Last 24 h', since: () => new Date(Date.now() - 86_400_000).toISOString() },
  { label: 'Last 7 d', since: () => new Date(Date.now() - 7 * 86_400_000).toISOString() },
  { label: 'Last 14 d', since: () => new Date(Date.now() - 14 * 86_400_000).toISOString() },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** True when the error is a "endpoint not found / service unavailable" signal. */
function isEndpointAbsent(err: unknown): boolean {
  if (err instanceof ApiError) {
    return err.status === 404 || err.status === 503;
  }
  return false;
}

/** Format an RFC 3339 string as "YYYY-MM-DD HH:mm UTC". */
function fmtTs(ts: string): string {
  const d = new Date(ts);
  if (isNaN(d.getTime())) return ts;
  return (
    d.toISOString().slice(0, 10) + ' ' + d.toISOString().slice(11, 16) + ' UTC'
  );
}

/**
 * Derive a simple 14-day fire-rate bucket array from a flat detection list.
 * Used as a fallback when no dedicated timeseries endpoint exists.
 */
function bucketDetections(
  detections: AnomalyDetection[],
  detectorId: string,
): AnomalyFireRateBucket[] {
  const counts = new Map<string, number>();
  const now = new Date();
  for (let i = 13; i >= 0; i--) {
    const d = new Date(now.getTime() - i * 86_400_000);
    counts.set(d.toISOString().slice(0, 10), 0);
  }
  for (const d of detections) {
    if (d.detector_id !== detectorId) continue;
    const day = d.fired_at.slice(0, 10);
    if (counts.has(day)) {
      counts.set(day, (counts.get(day) ?? 0) + 1);
    }
  }
  return Array.from(counts.entries()).map(([date, count]) => ({ date, count }));
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

interface DetectionRowProps {
  detection: AnomalyDetection;
  onFeedback: (id: string, isFp: boolean) => void;
  feedbackPending: boolean;
}

function DetectionRow({ detection, onFeedback, feedbackPending }: DetectionRowProps): JSX.Element {
  const { t } = useTranslation();
  const color = SEVERITY_COLOR[detection.severity];
  return (
    <tr>
      <td>{detection.detector_id}</td>
      <td>{fmtTs(detection.fired_at)}</td>
      <td>
        <span
          className="kind-tag"
          style={{ background: color, color: '#fff', fontWeight: 600 }}
          aria-label={t('pane.anomaly.severity', { severity: detection.severity })}
        >
          {detection.severity}
        </span>
      </td>
      <td>{detection.series_key}</td>
      <td style={{ textAlign: 'right' }}>{detection.value.toFixed(4)}</td>
      <td style={{ textAlign: 'right' }}>{detection.threshold.toFixed(4)}</td>
      <td>
        <button
          aria-label={t('pane.anomaly.btn_false_positive_aria')}
          title={
            detection.is_false_positive
              ? t('pane.anomaly.btn_undo_fp')
              : t('pane.anomaly.btn_false_positive')
          }
          disabled={feedbackPending}
          onClick={() => onFeedback(detection.id, !detection.is_false_positive)}
          style={{
            background: 'none',
            border: 'none',
            cursor: feedbackPending ? 'not-allowed' : 'pointer',
            fontSize: '1.1rem',
            opacity: detection.is_false_positive ? 0.4 : 1,
          }}
        >
          {detection.is_false_positive ? '👍' : '👎'}
        </button>
      </td>
    </tr>
  );
}

// ---------------------------------------------------------------------------
// Tuning panel
// ---------------------------------------------------------------------------

interface TuningPanelProps {
  config: AnomalyDetectorConfig;
  onApply: (patch: AnomalyDetectorPatch) => Promise<void>;
  applyPending: boolean;
}

function TuningPanel({ config, onApply, applyPending }: TuningPanelProps): JSX.Element {
  const { t } = useTranslation();

  // Local editable state — initialised from config, never mutates config.
  const [draft, setDraft] = useState<AnomalyDetectorKind>(() => ({ ...config.detector }));
  const [windowSecs, setWindowSecs] = useState(config.window_secs);
  const [coolOffSecs, setCoolOffSecs] = useState(config.cool_off_secs);

  // Reset when config changes (detector selection changed upstream).
  useEffect(() => {
    setDraft({ ...config.detector });
    setWindowSecs(config.window_secs);
    setCoolOffSecs(config.cool_off_secs);
  }, [config]);

  function updateDraftField(field: string, value: number): void {
    setDraft((prev) => ({ ...prev, [field]: value }));
  }

  async function handleApply(): Promise<void> {
    // HotL gate: changing detection thresholds affects audit posture.
    const confirmed = window.confirm(
      t('pane.anomaly.confirm_apply_tuning', { id: config.id }),
    );
    if (!confirmed) return;
    const patch: AnomalyDetectorPatch = {
      detector: draft,
      window_secs: windowSecs,
      cool_off_secs: coolOffSecs,
    };
    await onApply(patch);
  }

  return (
    <section
      aria-label={t('pane.anomaly.tuning_panel_aria', { id: config.id })}
      style={{
        border: '1px solid var(--border, #e5e7eb)',
        borderRadius: '6px',
        padding: '1rem',
        marginTop: '1rem',
      }}
    >
      <h3 style={{ fontSize: '0.95rem', fontWeight: 600, marginBottom: '0.75rem' }}>
        {t('pane.anomaly.tuning_title', { id: config.id })}
      </h3>

      <div style={{ display: 'grid', gap: '0.5rem', gridTemplateColumns: '1fr 1fr' }}>
        {/* sigma_threshold — present in both ZScore and EWMA */}
        {'sigma_threshold' in draft && (
          <label>
            {t('pane.anomaly.param_sigma')}
            <br />
            <input
              type="range"
              min={1}
              max={6}
              step={0.1}
              value={(draft as { sigma_threshold: number }).sigma_threshold}
              onChange={(e) => updateDraftField('sigma_threshold', parseFloat(e.target.value))}
              style={{ width: '100%' }}
            />
            <span style={{ fontSize: '0.8rem' }}>
              {(draft as { sigma_threshold: number }).sigma_threshold.toFixed(1)}σ
            </span>
          </label>
        )}

        {/* alpha — EWMA only */}
        {draft.kind === 'ewma' && (
          <label>
            {t('pane.anomaly.param_alpha')}
            <br />
            <input
              type="range"
              min={0.01}
              max={0.5}
              step={0.01}
              value={draft.alpha}
              onChange={(e) => updateDraftField('alpha', parseFloat(e.target.value))}
              style={{ width: '100%' }}
            />
            <span style={{ fontSize: '0.8rem' }}>{draft.alpha.toFixed(2)}</span>
          </label>
        )}

        {/* min_count */}
        {'min_count' in draft && (
          <label>
            {t('pane.anomaly.param_min_count')}
            <br />
            <input
              type="number"
              min={1}
              max={1000}
              value={(draft as { min_count: number }).min_count}
              onChange={(e) =>
                updateDraftField('min_count', parseInt(e.target.value, 10))
              }
              style={{ width: '80px' }}
            />
          </label>
        )}

        {/* window_secs */}
        <label>
          {t('pane.anomaly.param_window')}
          <br />
          <input
            type="number"
            min={60}
            step={60}
            value={windowSecs}
            onChange={(e) => setWindowSecs(parseInt(e.target.value, 10))}
            style={{ width: '100px' }}
          />
          <span style={{ fontSize: '0.8rem', marginLeft: '4px' }}>s</span>
        </label>

        {/* cool_off_secs */}
        <label>
          {t('pane.anomaly.param_cool_off')}
          <br />
          <input
            type="number"
            min={0}
            step={60}
            value={coolOffSecs}
            onChange={(e) => setCoolOffSecs(parseInt(e.target.value, 10))}
            style={{ width: '100px' }}
          />
          <span style={{ fontSize: '0.8rem', marginLeft: '4px' }}>s</span>
        </label>
      </div>

      <button
        onClick={() => void handleApply()}
        disabled={applyPending}
        style={{ marginTop: '0.75rem' }}
      >
        {applyPending
          ? t('pane.anomaly.btn_applying')
          : t('pane.anomaly.btn_apply_tuning')}
      </button>
      <span style={{ fontSize: '0.75rem', marginLeft: '0.5rem', color: '#6b7280' }}>
        {t('pane.anomaly.tuning_hotl_note')}
      </span>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Sparkline tile
// ---------------------------------------------------------------------------

interface SparklineTileProps {
  detectorId: string;
  buckets: AnomalyFireRateBucket[];
}

function SparklineTile({ detectorId, buckets }: SparklineTileProps): JSX.Element {
  const { t } = useTranslation();
  const maxCount = Math.max(...buckets.map((b) => b.count), 1);
  return (
    <div
      className="timeline-card timeline-card-chat"
      aria-label={t('pane.anomaly.sparkline_aria', { id: detectorId })}
      style={{ minWidth: '200px', flex: '1 1 200px' }}
    >
      <div className="timeline-card-body">
        <div className="timeline-card-row">
          <span className="kind-tag kind-tag-chat">{detectorId}</span>
        </div>
        <div className="timeline-card-meta">
          {t('pane.anomaly.sparkline_label')} · max {maxCount}
        </div>
        <ResponsiveContainer width="100%" height={60}>
          <LineChart data={buckets} margin={{ top: 2, right: 4, left: -36, bottom: 0 }}>
            <CartesianGrid strokeDasharray="2 2" vertical={false} />
            <XAxis dataKey="date" hide />
            <YAxis allowDecimals={false} tick={{ fontSize: 9 }} />
            <Tooltip
              formatter={(v: number) =>
                [`${v} fires`, '']
              }
              labelFormatter={(label: string) => label}
            />
            <Line
              type="monotone"
              dataKey="count"
              dot={false}
              stroke="#ef4444"
              strokeWidth={1.5}
            />
          </LineChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function AnomalyPane(): JSX.Element {
  const { t } = useTranslation();

  // Endpoint-absent sentinel: true when all /v1/anomaly/* return 404/503.
  const [endpointAbsent, setEndpointAbsent] = useState(false);

  const [detections, setDetections] = useState<AnomalyDetection[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Filters
  const [filterDetector, setFilterDetector] = useState('');
  const [filterSeverity, setFilterSeverity] = useState<AnomalySeverity | ''>('');
  const [filterRangeIdx, setFilterRangeIdx] = useState(1); // default: 7d

  // Selected detector for tuning
  const [selectedDetectorId, setSelectedDetectorId] = useState<string | null>(null);
  const [detectorConfig, setDetectorConfig] = useState<AnomalyDetectorConfig | null>(null);
  const [configLoading, setConfigLoading] = useState(false);
  const [applyPending, setApplyPending] = useState(false);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [applyOk, setApplyOk] = useState(false);

  // False-positive feedback
  const [feedbackPending, setFeedbackPending] = useState<Set<string>>(new Set());

  // Distinct detector IDs from loaded detections (for sparklines + selector).
  const detectorIds = Array.from(new Set(detections.map((d) => d.detector_id))).sort();

  // ---------------------------------------------------------------------------
  // Load detections
  // ---------------------------------------------------------------------------

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const since = TIME_RANGES[filterRangeIdx]?.since();
      const resp = await client.listAnomalyDetections({
        detector_id: filterDetector.trim() || undefined,
        severity: filterSeverity || undefined,
        since,
        limit: 100,
      });
      setDetections(resp.detections);
      setTotal(resp.total);
      setEndpointAbsent(false);
    } catch (err) {
      if (isEndpointAbsent(err)) {
        setEndpointAbsent(true);
        setDetections([]);
        setTotal(0);
      } else {
        setError((err as Error).message);
      }
    } finally {
      setLoading(false);
    }
  }, [filterDetector, filterSeverity, filterRangeIdx]);

  useEffect(() => {
    void load();
  }, [load]);

  // ---------------------------------------------------------------------------
  // Load detector config when selection changes
  // ---------------------------------------------------------------------------

  useEffect(() => {
    if (!selectedDetectorId) {
      setDetectorConfig(null);
      return;
    }
    let cancelled = false;
    setConfigLoading(true);
    setApplyError(null);
    setApplyOk(false);
    client
      .getAnomalyDetector(selectedDetectorId)
      .then((cfg) => {
        if (!cancelled) setDetectorConfig(cfg);
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setDetectorConfig(null);
          if (!isEndpointAbsent(err)) {
            setApplyError((err as Error).message);
          }
        }
      })
      .finally(() => {
        if (!cancelled) setConfigLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [selectedDetectorId]);

  // ---------------------------------------------------------------------------
  // Apply tuning (HotL-gated via window.confirm in TuningPanel)
  // ---------------------------------------------------------------------------

  async function handleApplyTuning(patch: AnomalyDetectorPatch): Promise<void> {
    if (!selectedDetectorId) return;
    setApplyPending(true);
    setApplyError(null);
    setApplyOk(false);
    try {
      const updated = await client.updateAnomalyDetector(selectedDetectorId, patch);
      setDetectorConfig(updated);
      setApplyOk(true);
    } catch (err) {
      setApplyError((err as Error).message);
    } finally {
      setApplyPending(false);
    }
  }

  // ---------------------------------------------------------------------------
  // False-positive feedback
  // ---------------------------------------------------------------------------

  async function handleFeedback(detectionId: string, isFp: boolean): Promise<void> {
    setFeedbackPending((prev) => new Set(prev).add(detectionId));
    try {
      await client.submitAnomalyFeedback({ detection_id: detectionId, is_false_positive: isFp });
      // Optimistically update local state — no refetch needed for a single field.
      setDetections((prev) =>
        prev.map((d) =>
          d.id === detectionId ? { ...d, is_false_positive: isFp } : d,
        ),
      );
    } catch {
      // Non-fatal; user can retry.
    } finally {
      setFeedbackPending((prev) => {
        const next = new Set(prev);
        next.delete(detectionId);
        return next;
      });
    }
  }

  // ---------------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------------

  return (
    <>
      <header className="today-header">
        <h1>{t('pane.anomaly.title')}</h1>
        <div className="today-meta">
          <button onClick={() => void load()} disabled={loading}>
            {loading ? t('common.loading') : t('common.refresh')}
          </button>
        </div>
      </header>

      {/* Endpoint-absent placeholder — shown when REST surface is not yet live */}
      {endpointAbsent && (
        <div
          className="empty"
          aria-live="polite"
          style={{
            border: '1px dashed #d1d5db',
            borderRadius: '6px',
            padding: '2rem',
            textAlign: 'center',
            color: '#6b7280',
            marginTop: '1rem',
          }}
        >
          <strong>{t('pane.anomaly.placeholder_title')}</strong>
          <br />
          {t('pane.anomaly.placeholder_body')}
        </div>
      )}

      {/* Only render the rest when endpoint appears to be available */}
      {!endpointAbsent && (
        <>
          {/* ── Filters ─────────────────────────────────────────────────── */}
          <div className="today-filters" role="group" aria-label={t('pane.anomaly.filters_aria')}>
            <label>
              {t('pane.anomaly.filter_detector')}
              <input
                className="search"
                value={filterDetector}
                onChange={(e) => setFilterDetector(e.target.value)}
                placeholder={t('pane.anomaly.filter_detector_placeholder')}
              />
            </label>
            <label>
              {t('pane.anomaly.filter_severity')}
              <select
                className="range"
                value={filterSeverity}
                onChange={(e) => setFilterSeverity(e.target.value as AnomalySeverity | '')}
              >
                {SEVERITY_OPTIONS.map((s) => (
                  <option key={s} value={s}>
                    {s === '' ? t('pane.anomaly.severity_all') : s}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t('pane.anomaly.filter_range')}
              <select
                className="range"
                value={filterRangeIdx}
                onChange={(e) => setFilterRangeIdx(parseInt(e.target.value, 10))}
              >
                {TIME_RANGES.map((r, i) => (
                  <option key={i} value={i}>
                    {r.label}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {error && (
            <div className="error">{t('common.failed', { message: error })}</div>
          )}

          {/* ── Detection list ───────────────────────────────────────────── */}
          {detections.length === 0 && !loading && !error && (
            <div className="empty">{t('pane.anomaly.empty_detections')}</div>
          )}

          {detections.length > 0 && (
            <section aria-label={t('pane.anomaly.detections_aria')} style={{ marginTop: '1rem' }}>
              <div style={{ fontSize: '0.85rem', color: '#6b7280', marginBottom: '0.4rem' }}>
                {t('pane.anomaly.detections_count', { count: detections.length, total })}
              </div>
              <div style={{ overflowX: 'auto' }}>
                <table className="usage-table">
                  <thead>
                    <tr>
                      <th scope="col">{t('pane.anomaly.col_detector')}</th>
                      <th scope="col">{t('pane.anomaly.col_fired_at')}</th>
                      <th scope="col">{t('pane.anomaly.col_severity')}</th>
                      <th scope="col">{t('pane.anomaly.col_series_key')}</th>
                      <th scope="col">{t('pane.anomaly.col_value')}</th>
                      <th scope="col">{t('pane.anomaly.col_threshold')}</th>
                      <th scope="col">{t('pane.anomaly.col_feedback')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {detections.map((d) => (
                      <DetectionRow
                        key={d.id}
                        detection={d}
                        onFeedback={handleFeedback}
                        feedbackPending={feedbackPending.has(d.id)}
                      />
                    ))}
                  </tbody>
                </table>
              </div>
            </section>
          )}

          {/* ── Per-detector tuning ──────────────────────────────────────── */}
          {detectorIds.length > 0 && (
            <section style={{ marginTop: '2rem' }}>
              <h2 style={{ fontSize: '1rem', fontWeight: 600, marginBottom: '0.5rem' }}>
                {t('pane.anomaly.tuning_section_title')}
              </h2>
              <label>
                {t('pane.anomaly.tuning_select_label')}
                <select
                  className="range"
                  value={selectedDetectorId ?? ''}
                  onChange={(e) =>
                    setSelectedDetectorId(e.target.value || null)
                  }
                  style={{ marginLeft: '0.5rem' }}
                >
                  <option value="">{t('pane.anomaly.tuning_select_placeholder')}</option>
                  {detectorIds.map((id) => (
                    <option key={id} value={id}>
                      {id}
                    </option>
                  ))}
                </select>
              </label>

              {configLoading && (
                <div className="empty">{t('common.loading')}</div>
              )}

              {applyError && (
                <div className="error">{t('common.failed', { message: applyError })}</div>
              )}
              {applyOk && (
                <div
                  style={{
                    color: '#16a34a',
                    fontSize: '0.85rem',
                    marginTop: '0.4rem',
                  }}
                  role="status"
                >
                  {t('pane.anomaly.apply_ok')}
                </div>
              )}

              {detectorConfig && !configLoading && (
                <TuningPanel
                  config={detectorConfig}
                  onApply={handleApplyTuning}
                  applyPending={applyPending}
                />
              )}
            </section>
          )}

          {/* ── 14-day sparkline tiles ───────────────────────────────────── */}
          {detectorIds.length > 0 && (
            <section style={{ marginTop: '2rem' }}>
              <h2 style={{ fontSize: '1rem', fontWeight: 600, marginBottom: '0.75rem' }}>
                {t('pane.anomaly.sparkline_section_title')}
              </h2>
              <div
                style={{
                  display: 'flex',
                  flexWrap: 'wrap',
                  gap: '0.75rem',
                }}
              >
                {detectorIds.map((id) => (
                  <SparklineTile
                    key={id}
                    detectorId={id}
                    buckets={bucketDetections(detections, id)}
                  />
                ))}
              </div>
            </section>
          )}
        </>
      )}
    </>
  );
}
