/**
 * feat(single-owner-ux) — Anomaly back-test pane.
 *
 * The single-binary build (DEC-033) ships exactly one live anomaly endpoint:
 * `POST /v1/anomaly/test`, which back-tests a declarative detector spec
 * against an inline CSV time-series and returns every point it would flag
 * (with the baseline mean/σ and z-score). That is what this pane drives — a
 * small form (detector + threshold + CSV, with sample buttons) → results
 * table — so an operator can see anomaly detection working *on the spot*
 * without waiting for a scheduled job.
 *
 * Live, continuous detection is a separate concern: it runs as a scheduled
 * job installed by a skill pack. The pane links out to the Scheduler (where
 * "Run now" can fire a check immediately) rather than re-implementing it.
 *
 * The earlier placeholder targeted a planned detections/detectors REST
 * surface that the embedded-SQLite build never grew; that dead path is
 * removed in favour of this working back-test.
 */

import { useCallback, useState } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import type {
  AnomalyBacktestRequest,
  AnomalyBacktestResponse,
  AnomalyDetectorKind,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import type { XiaoguaiClient } from '@xiaoguai/shared';
import { client as defaultClient } from '../client';
import { PaneIntro } from '../components/PaneIntro';
import { ErrorBanner } from '../components/ErrorBanner';

// ---------------------------------------------------------------------------
// Sample data sets (so a demo never needs the operator to hand-craft a CSV).
// ---------------------------------------------------------------------------

const SAMPLE_SPIKE = [
  'ts,value',
  '2026-06-01T00:00:00Z,100',
  '2026-06-01T01:00:00Z,102',
  '2026-06-01T02:00:00Z,99',
  '2026-06-01T03:00:00Z,101',
  '2026-06-01T04:00:00Z,100',
  '2026-06-01T05:00:00Z,103',
  '2026-06-01T06:00:00Z,98',
  '2026-06-01T07:00:00Z,101',
  '2026-06-01T08:00:00Z,100',
  '2026-06-01T09:00:00Z,102',
  '2026-06-01T10:00:00Z,5000',
  '2026-06-01T11:00:00Z,101',
].join('\n');

const SAMPLE_FLAT = [
  'ts,value',
  '2026-06-01T00:00:00Z,100',
  '2026-06-01T01:00:00Z,100',
  '2026-06-01T02:00:00Z,101',
  '2026-06-01T03:00:00Z,100',
  '2026-06-01T04:00:00Z,99',
  '2026-06-01T05:00:00Z,100',
  '2026-06-01T06:00:00Z,101',
  '2026-06-01T07:00:00Z,100',
  '2026-06-01T08:00:00Z,99',
  '2026-06-01T09:00:00Z,100',
  '2026-06-01T10:00:00Z,101',
  '2026-06-01T11:00:00Z,100',
].join('\n');

type DetectorChoice = 'z_score' | 'ewma';

/**
 * Build the wire detector union from the form fields. Pure: form state in,
 * `AnomalyDetectorKind` out (matches serde `rename_all = "snake_case"`).
 */
export function buildDetector(
  kind: DetectorChoice,
  sigma: number,
  alpha: number,
  minCount: number,
): AnomalyDetectorKind {
  if (kind === 'ewma') {
    return { kind: 'ewma', alpha, sigma_threshold: sigma, min_count: minCount };
  }
  return { kind: 'z_score', sigma_threshold: sigma, min_count: minCount };
}

/** Assemble the full back-test request body from the pane's form state. */
export function buildBacktestRequest(opts: {
  detector: AnomalyDetectorKind;
  csv: string;
  tsCol: string;
  valCol: string;
}): AnomalyBacktestRequest {
  return {
    spec: {
      id: 'backtest',
      kpi_query: 'n/a',
      window: 3600,
      detector: opts.detector,
      cool_off: 0,
      on_anomaly: { kind: 'notify', channel: 'ops' },
    },
    csv: opts.csv,
    ts_col: opts.tsCol,
    val_col: opts.valCol,
  };
}

export interface AnomalyPaneProps {
  /** Override the shared client (used by tests). */
  client?: Pick<XiaoguaiClient, 'anomalyBacktest'>;
}

export function AnomalyPane({ client }: AnomalyPaneProps = {}): JSX.Element {
  const c = client ?? defaultClient;
  const { t } = useTranslation();

  // Form state — never mutated in place.
  const [detector, setDetector] = useState<DetectorChoice>('z_score');
  const [sigma, setSigma] = useState(3);
  const [alpha, setAlpha] = useState(0.1);
  const [minCount, setMinCount] = useState(5);
  const [csv, setCsv] = useState(SAMPLE_SPIKE);
  const [tsCol, setTsCol] = useState('ts');
  const [valCol, setValCol] = useState('value');

  // Result state.
  const [result, setResult] = useState<AnomalyBacktestResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);

  const run = useCallback(async () => {
    setRunning(true);
    setError(null);
    setResult(null);
    try {
      const req = buildBacktestRequest({
        detector: buildDetector(detector, sigma, alpha, minCount),
        csv,
        tsCol: tsCol.trim(),
        valCol: valCol.trim(),
      });
      const resp = await c.anomalyBacktest(req);
      setResult(resp);
    } catch (err) {
      const msg = err instanceof ApiError ? err.message : (err as Error).message;
      setError(msg);
    } finally {
      setRunning(false);
    }
  }, [c, detector, sigma, alpha, minCount, csv, tsCol, valCol]);

  const csvEmpty = csv.trim().length === 0;

  return (
    <>
      <header className="anomaly-header">
        <h1>{t('pane.anomaly.title')}</h1>
      </header>

      <PaneIntro
        purpose={t('pane.anomaly.intro.purpose')}
        usage={t('pane.anomaly.intro.usage')}
        usageLabel={t('pane.anomaly.intro.usage_label')}
      />

      <section
        className="anomaly-form"
        aria-label={t('pane.anomaly.form_title')}
        data-testid="anomaly-form"
      >
        <h2>{t('pane.anomaly.form_title')}</h2>

        <div className="anomaly-form-grid">
          <label>
            {t('pane.anomaly.detector_label')}
            <select
              value={detector}
              onChange={(e) => setDetector(e.target.value as DetectorChoice)}
              data-testid="anomaly-detector"
            >
              <option value="z_score">{t('pane.anomaly.detector_zscore')}</option>
              <option value="ewma">{t('pane.anomaly.detector_ewma')}</option>
            </select>
          </label>

          <label>
            {t('pane.anomaly.sigma_label')}
            <input
              type="number"
              min={1}
              max={10}
              step={0.1}
              value={sigma}
              onChange={(e) => setSigma(parseFloat(e.target.value) || 0)}
              data-testid="anomaly-sigma"
            />
          </label>

          {detector === 'ewma' && (
            <label>
              {t('pane.anomaly.alpha_label')}
              <input
                type="number"
                min={0.01}
                max={1}
                step={0.01}
                value={alpha}
                onChange={(e) => setAlpha(parseFloat(e.target.value) || 0)}
                data-testid="anomaly-alpha"
              />
            </label>
          )}

          <label>
            {t('pane.anomaly.min_count_label')}
            <input
              type="number"
              min={1}
              max={1000}
              step={1}
              value={minCount}
              onChange={(e) => setMinCount(parseInt(e.target.value, 10) || 1)}
              data-testid="anomaly-min-count"
            />
          </label>

          <label>
            {t('pane.anomaly.ts_col_label')}
            <input
              type="text"
              value={tsCol}
              onChange={(e) => setTsCol(e.target.value)}
              data-testid="anomaly-ts-col"
            />
          </label>

          <label>
            {t('pane.anomaly.val_col_label')}
            <input
              type="text"
              value={valCol}
              onChange={(e) => setValCol(e.target.value)}
              data-testid="anomaly-val-col"
            />
          </label>
        </div>

        <div className="anomaly-samples">
          <span className="muted">{t('pane.anomaly.samples_label')}</span>
          <button
            type="button"
            onClick={() => setCsv(SAMPLE_SPIKE)}
            data-testid="anomaly-sample-spike"
          >
            {t('pane.anomaly.sample_spike')}
          </button>
          <button
            type="button"
            onClick={() => setCsv(SAMPLE_FLAT)}
            data-testid="anomaly-sample-flat"
          >
            {t('pane.anomaly.sample_flat')}
          </button>
        </div>

        <label className="anomaly-csv-label">
          {t('pane.anomaly.csv_label')}
          <textarea
            rows={8}
            value={csv}
            onChange={(e) => setCsv(e.target.value)}
            placeholder={t('pane.anomaly.csv_placeholder')}
            data-testid="anomaly-csv"
          />
        </label>

        <button
          type="button"
          className="anomaly-run-btn"
          onClick={() => void run()}
          disabled={running || csvEmpty}
          data-testid="anomaly-run"
        >
          {running ? t('pane.anomaly.btn_running') : t('pane.anomaly.btn_run')}
        </button>
      </section>

      <ErrorBanner message={error} />

      {result && (
        <section
          className="anomaly-result"
          aria-label={t('pane.anomaly.form_title')}
          data-testid="anomaly-result"
        >
          <p className="muted" data-testid="anomaly-summary">
            {t('pane.anomaly.result_summary', { summary: result.summary })}
          </p>
          {result.anomalies.length === 0 ? (
            <div className="empty" data-testid="anomaly-result-none">
              {t('pane.anomaly.result_none')}
            </div>
          ) : (
            <table className="anomaly-table" data-testid="anomaly-result-table">
              <thead>
                <tr>
                  <th>{t('pane.anomaly.col_ts')}</th>
                  <th>{t('pane.anomaly.col_value')}</th>
                  <th>{t('pane.anomaly.col_mean')}</th>
                  <th>{t('pane.anomaly.col_std')}</th>
                  <th>{t('pane.anomaly.col_score')}</th>
                  <th>{t('pane.anomaly.col_description')}</th>
                </tr>
              </thead>
              <tbody>
                {result.anomalies.map((a, i) => (
                  <tr key={`${a.ts}-${i}`} data-testid="anomaly-result-row">
                    <td>
                      <code>{a.ts}</code>
                    </td>
                    <td className="num">{a.value}</td>
                    <td className="num">{a.mean.toFixed(2)}</td>
                    <td className="num">{a.std.toFixed(2)}</td>
                    <td className="num">{a.score.toFixed(2)}</td>
                    <td>{a.description}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>
      )}

      <section className="anomaly-live" data-testid="anomaly-live">
        <h2>{t('pane.anomaly.live_title')}</h2>
        <p className="muted">{t('pane.anomaly.live_body')}</p>
        <Link to="/scheduler" data-testid="anomaly-scheduler-link">
          {t('pane.anomaly.live_link')}
        </Link>
      </section>
    </>
  );
}
