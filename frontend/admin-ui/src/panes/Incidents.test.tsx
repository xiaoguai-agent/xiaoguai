/**
 * DEC-040 — tests for the Incidents pane.
 *
 * Two layers, mirroring ExpertTeams.test.tsx:
 *   1. Pure helpers: form → DTO (trim + omit blanks), title validation,
 *      status-machine gates, latest-RCA pick.
 *   2. Component behaviour via a mock client: list renders, status filter
 *      passes the query param, create calls createIncident, the detail
 *      drawer drives analyze / approve (with confirm) / dismiss (with
 *      confirm), and a 503 shows the unavailable banner.
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nextProvider } from 'react-i18next';
import type {
  IncidentDetails,
  IncidentRecord,
  IncidentStatus,
  RcaRecord,
  RepairRecord,
} from '@xiaoguai/shared';
import i18n from '../i18n/index';
import { ScopeProvider, __resetFailOpenWarned } from '../hooks/useScopes';
import {
  IncidentsPane,
  EMPTY_INCIDENT_FORM,
  formToCreateIncidentReq,
  validateIncidentForm,
  isTerminalStatus,
  canAnalyze,
  canApprove,
  canDismiss,
  latestRca,
} from './Incidents';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const ID = '00000000-0000-0000-0000-0000000000c1';
const RCA_ID = '00000000-0000-0000-0000-0000000000r1';

function makeIncident(over: Partial<IncidentRecord> = {}): IncidentRecord {
  return {
    id: ID,
    source: 'manual',
    external_id: 'manual:disk-full',
    title: 'Disk full on backup host',
    severity: 'high',
    project: 'infra',
    environment: 'production',
    occurred_at: '2026-06-19T00:00:00Z',
    raw_payload: {},
    status: 'open',
    created_at: '2026-06-19T00:00:00Z',
    updated_at: '2026-06-19T00:00:00Z',
    ...over,
  };
}

function makeRca(over: Partial<RcaRecord> = {}): RcaRecord {
  return {
    id: RCA_ID,
    incident_id: ID,
    session_id: 'incident:1',
    summary: 'Disk filled by unrotated logs',
    root_cause: 'logrotate disabled',
    confidence: 0.8,
    action_items: [],
    raw_markdown: '# RCA',
    created_at: '2026-06-19T00:01:00Z',
    ...over,
  };
}

function makeRepair(over: Partial<RepairRecord> = {}): RepairRecord {
  return {
    id: 'rep1',
    incident_id: ID,
    rca_id: RCA_ID,
    session_id: 'incident:1',
    ok: true,
    summary: 'Re-enabled logrotate',
    created_at: '2026-06-19T00:02:00Z',
    ...over,
  };
}

function makeDetails(over: Partial<IncidentDetails> = {}): IncidentDetails {
  return {
    incident: makeIncident(),
    rcas: [],
    repairs: [],
    ...over,
  };
}

type ClientOverrides = Partial<{
  listIncidents: ReturnType<typeof vi.fn>;
  getIncident: ReturnType<typeof vi.fn>;
  createIncident: ReturnType<typeof vi.fn>;
  analyzeIncident: ReturnType<typeof vi.fn>;
  approveRepair: ReturnType<typeof vi.fn>;
  dismissIncident: ReturnType<typeof vi.fn>;
  incidentReport: ReturnType<typeof vi.fn>;
}>;

function makeClient(over: ClientOverrides = {}) {
  return {
    listIncidents: over.listIncidents ?? vi.fn(async () => [] as IncidentRecord[]),
    getIncident: over.getIncident ?? vi.fn(async () => makeDetails()),
    createIncident:
      over.createIncident ??
      vi.fn(async (req: { title: string; severity?: IncidentRecord['severity'] }) => ({
        incident: makeIncident({ title: req.title, severity: req.severity ?? 'medium' }),
        was_duplicate: false,
      })),
    analyzeIncident:
      over.analyzeIncident ??
      vi.fn(async () => ({ rca: makeRca(), status: 'awaiting_approval' as IncidentStatus })),
    approveRepair:
      over.approveRepair ??
      vi.fn(async () => ({ repair: makeRepair(), status: 'resolved' as IncidentStatus })),
    dismissIncident:
      over.dismissIncident ?? vi.fn(async () => makeIncident({ status: 'dismissed' })),
    incidentReport: over.incidentReport ?? vi.fn(async () => '# Incident report'),
  };
}

beforeEach(() => {
  __resetFailOpenWarned();
  if (typeof localStorage !== 'undefined') localStorage.clear();
});

function renderPane(client: ReturnType<typeof makeClient>) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ScopeProvider>
        <IncidentsPane client={client} />
      </ScopeProvider>
    </I18nextProvider>,
  );
}

// ---------------------------------------------------------------------------
// Pure-helper tests
// ---------------------------------------------------------------------------

describe('formToCreateIncidentReq', () => {
  it('trims the title and keeps the severity', () => {
    expect(
      formToCreateIncidentReq({ ...EMPTY_INCIDENT_FORM, title: '  boom ', severity: 'high' }),
    ).toEqual({ title: 'boom', severity: 'high' });
  });

  it('omits blank optional fields and includes populated ones', () => {
    expect(
      formToCreateIncidentReq({
        title: 'x',
        severity: 'low',
        project: '  infra ',
        environment: '   ',
      }),
    ).toEqual({ title: 'x', severity: 'low', project: 'infra' });
  });
});

describe('validateIncidentForm', () => {
  it('flags a blank title', () => {
    expect(validateIncidentForm(EMPTY_INCIDENT_FORM)).toBe('no_title');
    expect(validateIncidentForm({ ...EMPTY_INCIDENT_FORM, title: '   ' })).toBe('no_title');
  });
  it('passes with a title', () => {
    expect(validateIncidentForm({ ...EMPTY_INCIDENT_FORM, title: 'ok' })).toBeNull();
  });
});

describe('status-machine gates', () => {
  it('isTerminalStatus is true only for resolved/failed/dismissed', () => {
    expect(isTerminalStatus('resolved')).toBe(true);
    expect(isTerminalStatus('failed')).toBe(true);
    expect(isTerminalStatus('dismissed')).toBe(true);
    expect(isTerminalStatus('open')).toBe(false);
    expect(isTerminalStatus('awaiting_approval')).toBe(false);
  });
  it('canAnalyze only when open', () => {
    expect(canAnalyze('open')).toBe(true);
    expect(canAnalyze('analyzing')).toBe(false);
  });
  it('canApprove only when awaiting_approval', () => {
    expect(canApprove('awaiting_approval')).toBe(true);
    expect(canApprove('open')).toBe(false);
  });
  it('canDismiss for any non-terminal state', () => {
    expect(canDismiss('open')).toBe(true);
    expect(canDismiss('repairing')).toBe(true);
    expect(canDismiss('resolved')).toBe(false);
    expect(canDismiss('dismissed')).toBe(false);
  });
});

describe('latestRca', () => {
  it('returns the newest (first) RCA or null', () => {
    expect(latestRca(makeDetails({ rcas: [] }))).toBeNull();
    const newest = makeRca({ id: 'newest' });
    expect(latestRca(makeDetails({ rcas: [newest, makeRca({ id: 'older' })] }))?.id).toBe(
      'newest',
    );
  });
});

// ---------------------------------------------------------------------------
// Component behaviour
// ---------------------------------------------------------------------------

describe('<IncidentsPane>', () => {
  it('renders the incidents table', async () => {
    const client = makeClient({
      listIncidents: vi.fn(async () => [makeIncident({ title: 'Disk full on backup host' })]),
    });
    renderPane(client);
    await waitFor(() =>
      expect(screen.getByText('Disk full on backup host')).toBeTruthy(),
    );
    // Status chip rendered (en label for `open`) — scope to the table so the
    // toolbar's status-filter <option>Open</option> doesn't also match.
    expect(within(screen.getByRole('table')).getByText('Open')).toBeTruthy();
  });

  it('status filter passes the query param to listIncidents', async () => {
    const listIncidents = vi.fn(async () => [] as IncidentRecord[]);
    const client = makeClient({ listIncidents });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() => expect(listIncidents).toHaveBeenCalledWith(undefined));
    await user.selectOptions(screen.getByLabelText('Status'), 'awaiting_approval');
    await waitFor(() =>
      expect(listIncidents).toHaveBeenCalledWith('awaiting_approval'),
    );
  });

  it('create flow calls createIncident with the trimmed DTO', async () => {
    const createIncident = vi.fn(async (req) => ({
      incident: makeIncident({ title: req.title }),
      was_duplicate: false,
    }));
    const client = makeClient({ createIncident });
    const user = userEvent.setup();
    renderPane(client);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /new incident/i })).toBeTruthy(),
    );
    await user.click(screen.getByRole('button', { name: /new incident/i }));
    const dialog = await screen.findByRole('dialog');
    await user.type(
      within(dialog).getByPlaceholderText(/disk full on backup host/i),
      'Queue backed up',
    );
    await user.click(within(dialog).getByRole('button', { name: /create incident/i }));
    await waitFor(() => expect(createIncident).toHaveBeenCalled());
    expect(createIncident.mock.calls[0]![0]).toEqual({
      title: 'Queue backed up',
      severity: 'medium',
    });
  });

  it('view → analyze drives analyzeIncident', async () => {
    const analyzeIncident = vi.fn(async () => ({
      rca: makeRca(),
      status: 'awaiting_approval' as IncidentStatus,
    }));
    const client = makeClient({
      listIncidents: vi.fn(async () => [makeIncident({ status: 'open' })]),
      getIncident: vi.fn(async () => makeDetails({ incident: makeIncident({ status: 'open' }) })),
      analyzeIncident,
    });
    const user = userEvent.setup();
    renderPane(client);
    await user.click(await screen.findByLabelText(/view disk full/i));
    await user.click(await screen.findByRole('button', { name: /^analyze$/i }));
    await waitFor(() => expect(analyzeIncident).toHaveBeenCalledWith(ID));
  });

  it('approve flow confirms then calls approveRepair with the latest rca id', async () => {
    const approveRepair = vi.fn(async () => ({
      repair: makeRepair(),
      status: 'resolved' as IncidentStatus,
    }));
    const client = makeClient({
      listIncidents: vi.fn(async () => [makeIncident({ status: 'awaiting_approval' })]),
      getIncident: vi.fn(async () =>
        makeDetails({
          incident: makeIncident({ status: 'awaiting_approval' }),
          rcas: [makeRca()],
        }),
      ),
      approveRepair,
    });
    const user = userEvent.setup();
    renderPane(client);
    await user.click(await screen.findByLabelText(/view disk full/i));
    await user.click(await screen.findByRole('button', { name: /approve repair/i }));
    // Confirm modal overlays the detail drawer — click its button.
    const dialogs = await screen.findAllByRole('dialog');
    await user.click(
      within(dialogs[dialogs.length - 1]!).getByRole('button', { name: /approve repair/i }),
    );
    await waitFor(() => expect(approveRepair).toHaveBeenCalledWith(ID, RCA_ID));
  });

  it('dismiss flow confirms then calls dismissIncident', async () => {
    const dismissIncident = vi.fn(async () => makeIncident({ status: 'dismissed' }));
    const client = makeClient({
      listIncidents: vi.fn(async () => [makeIncident({ status: 'open' })]),
      getIncident: vi.fn(async () => makeDetails({ incident: makeIncident({ status: 'open' }) })),
      dismissIncident,
    });
    const user = userEvent.setup();
    renderPane(client);
    await user.click(await screen.findByLabelText(/view disk full/i));
    await user.click(await screen.findByRole('button', { name: /^dismiss$/i }));
    const dialogs = await screen.findAllByRole('dialog');
    await user.click(
      within(dialogs[dialogs.length - 1]!).getByRole('button', { name: /^dismiss$/i }),
    );
    await waitFor(() => expect(dismissIncident).toHaveBeenCalledWith(ID));
  });

  it('shows the 503 banner when listIncidents throws ApiError(503)', async () => {
    const { ApiError } = await import('@xiaoguai/shared');
    const client = makeClient({
      listIncidents: vi.fn(async () => {
        throw new ApiError(503, 'unavailable', 'no store');
      }),
    });
    renderPane(client);
    await waitFor(() =>
      expect(screen.getByText(/incident store not configured/i)).toBeTruthy(),
    );
  });
});
