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
import { useTranslation } from 'react-i18next';
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
  const { t } = useTranslation();
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
        <h1>{t('pane.eval.title')}</h1>
        <p className="muted">{t('pane.eval.description')}</p>
      </header>

      <div className="eval-layout">
        <aside className="eval-sidebar">
          <div className="eval-sidebar-head">
            <h2>{t('pane.eval.suites_title')}</h2>
            <button type="button" onClick={() => void refreshSuites()}>
              {t('common.refresh')}
            </button>
          </div>
          {suitesError && <div className="error">{t('common.failed', { message: suitesError })}</div>}
          {suites === null ? (
            <div className="empty">{t('pane.eval.suites_empty_loading')}</div>
          ) : suites.length === 0 ? (
            <div className="empty">{t('pane.eval.suites_empty_none')}</div>
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
                        ? t('pane.eval.suite_single_file')
                        : t(
                            s.case_count === 1
                              ? 'pane.eval.suite_case_count_one'
                              : 'pane.eval.suite_case_count_other',
                            { count: s.case_count },
                          )}
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
              {t('pane.eval.tab_run_suite')}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'convert'}
              className={tab === 'convert' ? 'active' : ''}
              onClick={() => setTab('convert')}
            >
              {t('pane.eval.tab_convert')}
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
  const { t } = useTranslation();
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
      <div className="empty">{t('pane.eval.run_pick_prompt')}</div>
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
          {running ? <Spinner /> : t('pane.eval.run_btn')}
        </button>
        <span className="muted">
          {t('pane.eval.run_suite_label')} <code>{suiteName}</code>
        </span>
        {report && (
          <span className="muted">
            {t('pane.eval.run_last_run', { time: new Date(report.finished_at).toLocaleString() })}
          </span>
        )}
      </div>

      {error && <div className="error">{t('common.failed', { message: error })}</div>}

      {report && (
        <div className="run-report">
          <div className={`pass-rate-card ${passed === total ? 'all-pass' : ''}`}>
            <div className="rate">{Math.round(report.pass_rate * 100)}%</div>
            <div className="rate-meta">
              {t('pane.eval.run_passed', { passed, total })}
            </div>
          </div>
          <table className="result-table">
            <thead>
              <tr>
                <th>{t('pane.eval.col_case')}</th>
                <th>{t('pane.eval.col_status')}</th>
                <th className="num">{t('pane.eval.col_duration')}</th>
                <th className="num">{t('pane.eval.col_events')}</th>
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
  const { t } = useTranslation();
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
      <p className="muted">{t('pane.eval.convert_description')}</p>
      <div className="convert-input">
        <input
          type="text"
          value={sessionId}
          onChange={(e) => setSessionId(e.target.value)}
          placeholder={t('pane.eval.convert_placeholder')}
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
          {loading ? <Spinner /> : t('pane.eval.convert_btn')}
        </button>
      </div>
      {error && <div className="error">{t('common.failed', { message: error })}</div>}
      {resp && (
        <div className="convert-output">
          <div className="convert-meta">
            <span>
              {t('pane.eval.suggested_filename')} <code>{resp.suggested_filename}</code>
            </span>
            <span className="muted">
              {t(
                resp.tool_invocation_count === 1
                  ? 'pane.eval.convert_tool_invocations_one'
                  : 'pane.eval.convert_tool_invocations_other',
                { count: resp.tool_invocation_count },
              )}
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
