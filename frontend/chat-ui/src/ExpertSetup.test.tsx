/**
 * ExpertSetup — the per-expert prerequisite checklist page (v1.34).
 *
 * Covers:
 *  - Required groups render; an unsatisfied MCP item installs inline
 *    (installMarketplace → readiness refetch flips it to ✓).
 *  - A package item shows the host-install command + a copy button and an
 *    honest "not found" probe status — NO install button (can't install a host
 *    package from the browser).
 *  - Optional add-ons render with install buttons.
 *  - Unknown expert key → the unknown-expert message.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import type { ExpertReadinessResponse } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';
import { ExpertSetup } from './ExpertSetup';

let routeKey = 'vmware-ops';
vi.mock('react-router-dom', async (orig) => {
  const actual = await orig<typeof import('react-router-dom')>();
  return { ...actual, useParams: () => ({ key: routeKey }), useNavigate: () => vi.fn() };
});

vi.mock('./client', () => ({
  client: { listExperts: vi.fn(), installMarketplace: vi.fn() },
}));
import { client } from './client';
const mc = vi.mocked(client);

function resp(opts: { monitorInstalled: boolean; policyOk: boolean }): ExpertReadinessResponse {
  return {
    version: 1,
    offline_hint: '离线:设 UV_INDEX_URL',
    offline_hint_en: 'Offline: set UV_INDEX_URL',
    experts: [
      {
        key: 'vmware-ops',
        persona_name: 'VMware 运维助手',
        name: 'VMware Ops',
        name_zh: 'VMware 运维助手',
        summary: null,
        summary_zh: null,
        ready: opts.policyOk && opts.monitorInstalled,
        required: [
          {
            label: 'Policy foundation',
            label_zh: '策略与审计基座',
            satisfied: opts.policyOk,
            any_of: [
              {
                kind: 'package',
                id: 'vmware-policy',
                label: 'vmware-policy',
                satisfied: opts.policyOk,
                install: 'uv tool install vmware-policy',
              },
            ],
          },
          {
            label: 'Capability server',
            label_zh: '能力服务器',
            satisfied: opts.monitorInstalled,
            any_of: [
              { kind: 'mcp', id: 'vmware-monitor', label: 'VMware Monitor', satisfied: opts.monitorInstalled, install: 'vmware-monitor' },
              { kind: 'mcp', id: 'vmware-aiops', label: 'VMware AIOps', satisfied: false, install: 'vmware-aiops' },
            ],
          },
        ],
        optional: [
          { slug: 'vmware-storage', name: 'VMware Storage', name_zh: null, installed: false },
        ],
      },
    ],
  };
}

function renderSetup() {
  return render(
    <I18nProvider>
      <ExpertSetup />
    </I18nProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  routeKey = 'vmware-ops';
  mc.installMarketplace.mockResolvedValue({} as never);
});

describe('ExpertSetup', () => {
  it('installs an unsatisfied MCP item inline and refetches readiness', async () => {
    // First load: monitor NOT installed; after install, refetch shows it satisfied.
    mc.listExperts
      .mockResolvedValueOnce(resp({ monitorInstalled: false, policyOk: true }))
      .mockResolvedValueOnce(resp({ monitorInstalled: true, policyOk: true }));
    renderSetup();
    await waitFor(() => expect(screen.getByText('VMware Monitor')).toBeTruthy());

    const monitorRow = screen.getByText('VMware Monitor').closest('.expert-setup__item') as HTMLElement;
    fireEvent.click(within(monitorRow).getByRole('button'));
    await waitFor(() =>
      expect(mc.installMarketplace).toHaveBeenCalledWith({ slug: 'vmware-monitor' }),
    );
    // Refetch flipped it to installed → the ✓ shows.
    await waitFor(() => {
      const row = screen.getByText('VMware Monitor').closest('.expert-setup__item') as HTMLElement;
      expect(row.querySelector('.expert-setup__ok')).toBeTruthy();
    });
  });

  it('shows a host-install command + copy for a package item, no install button', async () => {
    mc.listExperts.mockResolvedValue(resp({ monitorInstalled: true, policyOk: false }));
    renderSetup();
    await waitFor(() => expect(screen.getByText('vmware-policy')).toBeTruthy());
    const row = screen.getByText('vmware-policy').closest('.expert-setup__item') as HTMLElement;
    // The exact host command is shown.
    expect(within(row).getByText(/uv tool install vmware-policy/)).toBeTruthy();
    // Honest "not found" + a copy button, but NO Install button for a package.
    expect(within(row).getByText(/not found|未检测到/)).toBeTruthy();
    expect(within(row).queryByText(/^Install$|^安装$/)).toBeNull();
    expect(mc.installMarketplace).not.toHaveBeenCalled();
  });

  it('renders an unknown expert message for a bad key', async () => {
    routeKey = 'nope';
    mc.listExperts.mockResolvedValue(resp({ monitorInstalled: true, policyOk: true }));
    renderSetup();
    await waitFor(() => expect(screen.getByText(/Unknown expert|未知的专家/)).toBeTruthy());
  });
});
