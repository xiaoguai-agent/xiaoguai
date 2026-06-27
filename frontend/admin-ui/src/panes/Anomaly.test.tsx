/**
 * feat(single-owner-ux) — tests for the Anomaly back-test pane.
 *
 * Two layers:
 *   1. Pure helpers: buildDetector (z_score / ewma union shape) and
 *      buildBacktestRequest (full spec assembly).
 *   2. Component behaviour via a mock client: the form renders, a sample
 *      button fills the CSV, Run back-test POSTs a well-formed request and
 *      renders the result rows + summary; the EWMA option reveals the alpha
 *      field; a 400 surfaces the error banner; and the live-detection
 *      section links to the Scheduler.
 *
 * The pane renders a react-router <Link>, so it is wrapped in a
 * <MemoryRouter>.
 */

import { describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import { MemoryRouter } from 'react-router-dom';
import { ApiError } from '@xiaoguai/shared';
import type {
  AnomalyBacktestRequest,
  AnomalyBacktestResponse,
} from '@xiaoguai/shared';
import i18n from '../i18n/index';
import {
  AnomalyPane,
  buildDetector,
  buildBacktestRequest,
} from './Anomaly';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const RESULT: AnomalyBacktestResponse = {
  anomalies: [
    {
      ts: '2026-06-01T10:00:00+00:00',
      value: 5000,
      mean: 100.6,
      std: 1.5,
      score: 3265.6,
      description: 'value 5000 is 3265.6σ above mean 100.6',
    },
  ],
  summary: '1 anomalies in 12 points (detector: zscore)',
};

const EMPTY_RESULT: AnomalyBacktestResponse = {
  anomalies: [],
  summary: '0 anomalies in 12 points (detector: zscore)',
};

function makeClient(backtest?: ReturnType<typeof vi.fn>) {
  return {
    anomalyBacktest: backtest ?? vi.fn(async () => RESULT),
  };
}

function renderPane(client: ReturnType<typeof makeClient>) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter>
        <AnomalyPane client={client} />
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

describe('buildDetector', () => {
  it('builds a z_score detector without alpha', () => {
    const d = buildDetector('z_score', 3, 0.1, 5);
    expect(d).toEqual({ kind: 'z_score', sigma_threshold: 3, min_count: 5 });
  });

  it('builds an ewma detector carrying alpha', () => {
    const d = buildDetector('ewma', 2.5, 0.2, 8);
    expect(d).toEqual({
      kind: 'ewma',
      alpha: 0.2,
      sigma_threshold: 2.5,
      min_count: 8,
    });
  });
});

describe('buildBacktestRequest', () => {
  it('assembles a complete spec with the csv + column names', () => {
    const req = buildBacktestRequest({
      detector: { kind: 'z_score', sigma_threshold: 3, min_count: 5 },
      csv: 'ts,value\n0,100',
      tsCol: 'ts',
      valCol: 'value',
    });
    expect(req.csv).toBe('ts,value\n0,100');
    expect(req.ts_col).toBe('ts');
    expect(req.val_col).toBe('value');
    expect(req.spec.detector).toEqual({
      kind: 'z_score',
      sigma_threshold: 3,
      min_count: 5,
    });
    // A complete spec is required by the backend even for a back-test.
    expect(req.spec.id).toBeTruthy();
    expect(req.spec.on_anomaly).toEqual({ kind: 'notify', channel: 'ops' });
    expect(typeof req.spec.window).toBe('number');
    expect(typeof req.spec.cool_off).toBe('number');
  });
});

// ---------------------------------------------------------------------------
// Component behaviour
// ---------------------------------------------------------------------------

describe('<AnomalyPane>', () => {
  it('renders the back-test form and the live-detection link to the Scheduler', () => {
    renderPane(makeClient());
    expect(screen.getByTestId('anomaly-form')).toBeInTheDocument();
    expect(screen.getByTestId('anomaly-csv')).toBeInTheDocument();
    const link = screen.getByTestId('anomaly-scheduler-link');
    expect(link).toHaveAttribute('href', '/scheduler');
  });

  it('Run back-test POSTs a well-formed request and renders result rows + summary', async () => {
    const backtest = vi.fn(async (_req: AnomalyBacktestRequest) => RESULT);
    renderPane(makeClient(backtest));

    await userEvent.click(screen.getByTestId('anomaly-run'));

    await waitFor(() => expect(backtest).toHaveBeenCalledTimes(1));
    const req = backtest.mock.calls[0]![0];
    expect(req.ts_col).toBe('ts');
    expect(req.val_col).toBe('value');
    expect(req.spec.detector.kind).toBe('z_score');
    expect(req.csv).toContain('5000'); // default sample is the spike set

    // Results render: summary + one anomaly row.
    expect(screen.getByTestId('anomaly-summary')).toHaveTextContent('1 anomalies');
    const rows = screen.getAllByTestId('anomaly-result-row');
    expect(rows).toHaveLength(1);
    expect(within(rows[0]!).getByText('5000')).toBeInTheDocument();
  });

  it('shows the no-anomalies empty result when none fire', async () => {
    const backtest = vi.fn(async () => EMPTY_RESULT);
    renderPane(makeClient(backtest));

    await userEvent.click(screen.getByTestId('anomaly-run'));
    await waitFor(() =>
      expect(screen.getByTestId('anomaly-result-none')).toBeInTheDocument(),
    );
    expect(screen.queryByTestId('anomaly-result-table')).toBeNull();
  });

  it('reveals the alpha field and sends an ewma detector when EWMA is selected', async () => {
    const backtest = vi.fn(async (_req: AnomalyBacktestRequest) => RESULT);
    renderPane(makeClient(backtest));

    // alpha hidden for z_score (the default).
    expect(screen.queryByTestId('anomaly-alpha')).toBeNull();

    await userEvent.selectOptions(screen.getByTestId('anomaly-detector'), 'ewma');
    expect(screen.getByTestId('anomaly-alpha')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('anomaly-run'));
    await waitFor(() => expect(backtest).toHaveBeenCalledTimes(1));
    expect(backtest.mock.calls[0]![0].spec.detector.kind).toBe('ewma');
  });

  it('loads the flat sample when its button is clicked', async () => {
    renderPane(makeClient());
    const csv = screen.getByTestId('anomaly-csv') as HTMLTextAreaElement;
    expect(csv.value).toContain('5000'); // spike sample by default

    await userEvent.click(screen.getByTestId('anomaly-sample-flat'));
    expect(csv.value).not.toContain('5000');
  });

  it('surfaces a 400 from the backend in the error banner', async () => {
    const backtest = vi.fn(async () => {
      throw new ApiError(400, 'bad_request', 'CSV parse error: line 3');
    });
    renderPane(makeClient(backtest));

    await userEvent.click(screen.getByTestId('anomaly-run'));
    await waitFor(() =>
      expect(screen.getByText(/CSV parse error/)).toBeInTheDocument(),
    );
  });
});
