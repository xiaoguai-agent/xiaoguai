/**
 * v0.11.2 — Eval pane.
 *
 * Closes the audit ↔ eval loop the roadmap §3 v0.11.2 row sketches:
 *   - left column: suites discoverable on disk (from `/v1/admin/eval/suites`)
 *   - right column, "Run" tab: run the selected suite, render pass rate +
 *     per-case rows. Last-run cached in localStorage so a refresh keeps
 *     the verdict visible.
 *   - right column, "Convert from session" tab: take a `sessions.id`, hand
 *     back a paste-ready `.eval.yaml` string.
 *
 * Browser verification deferred to human — same caveat as every admin-ui
 * tag since v0.8.1.
 */

import { useCallback, useEffect, useState } from 'react';
import type {
  CaseFromSessionResponse,
  EvalReport,
  EvalResult,
  EvalSuiteListItem,
} from '@xiaoguai/shared';
import { client } from '../client';
import { CopyButton } from '../components/CopyButton';

type RightTab = 'run' | 'convert';

const LAST_RUN_KEY = (suite: string) => `xiaoguai.eval.lastRun.${suite}`;

export function EvalPane() {
  const [suites, setSuites] = useState<EvalSuiteListItem[] | null>(null);
  const [suitesError, setSuitesError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [tab, setTab] = useState<RightTab>('run');

  const refreshSuites = useCallback(async () => {
    setSuitesError(null);
    try {
      const got = await client.listEvalSuites();
      setSuites(got);
      if (got.length > 0 && selected === null) {
        const first = got[0];
        if (first) setSelected(first.name);
      }
    } catch (err) {
      setSuitesError((err as Error).message);
    }
  }, [selected]);

  useEffect(() => {
    void refreshSuites();
  }, [refreshSuites]);

  return (
    <>
      <header className="eval-header">
        <h1>Eval</h1>
        <p className="muted">
          Regression + capability suites against the same{' '}
          <code>MockBackend</code> the CLI uses. Reports stay on disk; the
          console caches the last verdict per suite.
        </p>
      </header>

      <div className="eval-layout">
        <aside className="eval-sidebar">
          <div className="eval-sidebar-head">
            <h2>Suites</h2>
            <button type="button" onClick={() => void refreshSuites()}>
              Refresh
            </button>
          </div>
          {suitesError && <div className="error">Failed: {suitesError}</div>}
          {suites === null ? (
            <div className="empty">Loading…</div>
          ) : suites.length === 0 ? (
            <div className="empty">
              No suites under the configured directory.
            </div>
          ) : (
            <ul className="eval-suite-list">
              {suites.map((s) => (
                <li key={s.name}>
                  <button
                    type="button"
                    className={selected === s.name ? 'active' : ''}
                    onClick={() => setSelected(s.name)}
                  >
                    <span className="suite-name">{s.name}</span>
                    <span className="suite-meta">
                      {s.case_count === null
                        ? 'single file'
                        : `${s.case_count} case${s.case_count === 1 ? '' : 's'}`}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </aside>

        <section className="eval-detail">
          <div className="eval-tabs" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'run'}
              className={tab === 'run' ? 'active' : ''}
              onClick={() => setTab('run')}
            >
              Run suite
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'convert'}
              className={tab === 'convert' ? 'active' : ''}
              onClick={() => setTab('convert')}
            >
              Convert from session
            </button>
          </div>

          {tab === 'run' ? (
            <RunSuiteTab suiteName={selected} />
          ) : (
            <ConvertFromSessionTab />
          )}
        </section>
      </div>
    </>
  );
}

function RunSuiteTab({ suiteName }: { suiteName: string | null }): JSX.Element {
  const [report, setReport] = useState<EvalReport | null>(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Restore the last-run verdict per suite so refreshing the page doesn't
  // lose what the operator just observed.
  useEffect(() => {
    if (suiteName === null) {
      setReport(null);
      setError(null);
      return;
    }
    try {
      const raw = window.localStorage.getItem(LAST_RUN_KEY(suiteName));
      setReport(raw ? (JSON.parse(raw) as EvalReport) : null);
      setError(null);
    } catch {
      setReport(null);
    }
  }, [suiteName]);

  const run = useCallback(async () => {
    if (suiteName === null) return;
    setRunning(true);
    setError(null);
    try {
      const r = await client.runEvalSuite({ suite_name: suiteName });
      setReport(r);
      try {
        window.localStorage.setItem(LAST_RUN_KEY(suiteName), JSON.stringify(r));
      } catch {
        // Quota / private mode — fine, report still in state.
      }
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setRunning(false);
    }
  }, [suiteName]);

  if (suiteName === null) {
    return (
      <div className="empty">Pick a suite from the left to run it.</div>
    );
  }

  const passed = report ? report.results.filter((r) => r.status === 'pass').length : 0;
  const total = report?.results.length ?? 0;

  return (
    <div className="run-tab">
      <div className="run-actions">
        <button
          type="button"
          className="run-btn"
          onClick={() => void run()}
          disabled={running}
        >
          {running ? <Spinner /> : 'Run suite'}
        </button>
        <span className="muted">
          Suite: <code>{suiteName}</code>
        </span>
        {report && (
          <span className="muted">
            Last run: {new Date(report.finished_at).toLocaleString()}
          </span>
        )}
      </div>

      {error && <div className="error">Failed: {error}</div>}

      {report && (
        <div className="run-report">
          <div className={`pass-rate-card ${passed === total ? 'all-pass' : ''}`}>
            <div className="rate">{Math.round(report.pass_rate * 100)}%</div>
            <div className="rate-meta">
              {passed} / {total} passed
            </div>
          </div>
          <table className="result-table">
            <thead>
              <tr>
                <th>Case</th>
                <th>Status</th>
                <th className="num">Duration</th>
                <th className="num">Events</th>
              </tr>
            </thead>
            <tbody>
              {report.results.map((r) => (
                <ResultRow key={r.case_id} row={r} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function ResultRow({ row }: { row: EvalResult }): JSX.Element {
  const [expanded, setExpanded] = useState(false);
  const isFail = row.status === 'fail';
  return (
    <>
      <tr className={isFail ? 'fail-row' : 'pass-row'}>
        <td>
          {isFail && row.reasons && row.reasons.length > 0 ? (
            <button
              type="button"
              className="expand-btn"
              onClick={() => setExpanded((v) => !v)}
              aria-expanded={expanded}
            >
              {expanded ? '▾' : '▸'} {row.case_id}
            </button>
          ) : (
            row.case_id
          )}
        </td>
        <td>
          <span className={`status-badge status-${row.status}`}>
            {row.status.toUpperCase()}
          </span>
        </td>
        <td className="num">{row.duration_ms} ms</td>
        <td className="num">{row.transcript_len}</td>
      </tr>
      {expanded && row.reasons && (
        <tr className="fail-detail">
          <td colSpan={4}>
            <ul>
              {row.reasons.map((r, i) => (
                <li key={i}>{r}</li>
              ))}
            </ul>
          </td>
        </tr>
      )}
    </>
  );
}

function ConvertFromSessionTab(): JSX.Element {
  const [sessionId, setSessionId] = useState('');
  const [resp, setResp] = useState<CaseFromSessionResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const convert = useCallback(async () => {
    if (!sessionId.trim()) return;
    setLoading(true);
    setError(null);
    setResp(null);
    try {
      const r = await client.evalCaseFromSession({
        session_id: sessionId.trim(),
      });
      setResp(r);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  return (
    <div className="convert-tab">
      <p className="muted">
        Project a production <code>sessions.id</code> into a ready-to-edit{' '}
        <code>.eval.yaml</code> case. The server suggests assertions; review
        + paste into a file under <code>suites_dir/</code>.
      </p>
      <div className="convert-input">
        <input
          type="text"
          value={sessionId}
          onChange={(e) => setSessionId(e.target.value)}
          placeholder="sessions.id (e.g. sess_abc123)"
          spellCheck={false}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void convert();
          }}
        />
        <button
          type="button"
          onClick={() => void convert()}
          disabled={loading || !sessionId.trim()}
        >
          {loading ? <Spinner /> : 'Convert'}
        </button>
      </div>
      {error && <div className="error">Failed: {error}</div>}
      {resp && (
        <div className="convert-output">
          <div className="convert-meta">
            <span>
              Suggested filename: <code>{resp.suggested_filename}</code>
            </span>
            <span className="muted">
              {resp.tool_invocation_count} tool invocation
              {resp.tool_invocation_count === 1 ? '' : 's'}
            </span>
          </div>
          <div className="yaml-block">
            <CopyButton text={resp.case_yaml} />
            <pre>
              <code>{resp.case_yaml}</code>
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}

function Spinner(): JSX.Element {
  return <span className="spinner" aria-hidden="true" />;
}
