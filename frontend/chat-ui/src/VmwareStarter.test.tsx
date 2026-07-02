/**
 * VmwareStarter tests — the one-click VM-ops starter card.
 *
 * Covers:
 *  - Loads the marketplace, filters to `vmware-*` slugs, orders monitor →
 *    aiops → rest (catalog order preserved for the tail)
 *  - Read-only / ops badges on monitor / aiops rows
 *  - Already-installed servers (matched by name) pre-mark their row
 *  - One-click install calls installMarketplace with the slug and flips the
 *    row to installed (button disabled)
 *  - Install failure surfaces the inline error and stays actionable
 *  - Catalog load failure renders the inline error state
 *  - zh-CN locale prefers `name_zh` with fallback to `name`
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import type { MarketplaceEntry, McpServerResponse } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';
import { VmwareStarter } from './VmwareStarter';

// Mock the client module so nothing hits the network.
vi.mock('./client', () => ({
  client: {
    listMarketplace: vi.fn(),
    listMcpServers: vi.fn(),
    installMarketplace: vi.fn(),
  },
}));
import { client } from './client';
const mockedClient = vi.mocked(client);

/** Minimal marketplace entry; overrides fill per-test details. */
function entry(overrides: Partial<MarketplaceEntry> & { slug: string }): MarketplaceEntry {
  return {
    name: overrides.slug,
    description: `desc of ${overrides.slug}`,
    category: 'ops',
    transport: 'stdio',
    version: '1.7.0',
    command: `${overrides.slug}-mcp`,
    args: [],
    env_keys: [],
    ...overrides,
  };
}

/** The catalog under test: 2 non-vmware decoys + 3 vmware in NON-preferred order. */
const CATALOG: MarketplaceEntry[] = [
  entry({ slug: 'github', name: 'GitHub' }),
  entry({ slug: 'vmware-storage', name: 'VMware Storage' }),
  entry({ slug: 'vmware-aiops', name: 'VMware AIops', name_zh: 'VMware 运维' }),
  entry({ slug: 'vmware-monitor', name: 'VMware Monitor', name_zh: 'VMware 监控' }),
  entry({ slug: 'sqlite', name: 'SQLite' }),
];

function server(name: string): McpServerResponse {
  return {
    id: `srv-${name}`,
    name,
    version: '1.7.0',
    transport: 'stdio',
    command: `${name}-mcp`,
    args: [],
    env_keys: [],
    endpoint: null,
  };
}

function renderStarter() {
  return render(
    <I18nProvider>
      <VmwareStarter />
    </I18nProvider>,
  );
}

/** Wait until the row list has rendered (loading state replaced). */
async function renderSettled() {
  const result = renderStarter();
  await waitFor(() =>
    expect(screen.getAllByTestId(/^vmware-install-/).length).toBeGreaterThan(0),
  );
  return result;
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  mockedClient.listMarketplace.mockResolvedValue({ version: 1, entries: CATALOG });
  mockedClient.listMcpServers.mockResolvedValue([]);
  mockedClient.installMarketplace.mockResolvedValue({} as never);
});

describe('VmwareStarter', () => {
  it('filters to vmware-* slugs and orders monitor → aiops → rest', async () => {
    await renderSettled();
    const buttons = screen.getAllByTestId(/^vmware-install-/);
    expect(buttons.map((b) => b.getAttribute('data-testid'))).toEqual([
      'vmware-install-vmware-monitor',
      'vmware-install-vmware-aiops',
      'vmware-install-vmware-storage',
    ]);
    // Decoys never render.
    expect(screen.queryByText('GitHub')).toBeNull();
    expect(screen.queryByText('SQLite')).toBeNull();
  });

  it('shows the read-only badge on monitor and the ops badge on aiops', async () => {
    await renderSettled();
    const rows = screen.getAllByRole('listitem');
    expect(rows).toHaveLength(3);
    expect(within(rows[0]!).getByText(/read-only/i)).toBeTruthy();
    expect(within(rows[1]!).getByText(/destructive ops/i)).toBeTruthy();
    // The tail row carries no badge.
    expect(rows[2]!.querySelector('.vmware-starter__badge')).toBeNull();
  });

  it('pre-marks already-installed servers (matched by name) as installed', async () => {
    mockedClient.listMcpServers.mockResolvedValue([server('VMware Monitor')]);
    await renderSettled();
    const monitorBtn = screen.getByTestId('vmware-install-vmware-monitor');
    expect(monitorBtn).toBeDisabled();
    expect(monitorBtn.textContent).toContain('Installed');
    // Others stay actionable.
    expect(screen.getByTestId('vmware-install-vmware-aiops')).toBeEnabled();
  });

  it('one-click install calls installMarketplace with the slug and flips the row', async () => {
    await renderSettled();
    fireEvent.click(screen.getByTestId('vmware-install-vmware-monitor'));
    await waitFor(() =>
      expect(mockedClient.installMarketplace).toHaveBeenCalledWith({
        slug: 'vmware-monitor',
      }),
    );
    await waitFor(() => {
      const btn = screen.getByTestId('vmware-install-vmware-monitor');
      expect(btn).toBeDisabled();
      expect(btn.textContent).toContain('Installed');
    });
    // Only the clicked row installed.
    expect(mockedClient.installMarketplace).toHaveBeenCalledTimes(1);
  });

  it('surfaces an install failure inline and stays actionable', async () => {
    mockedClient.installMarketplace.mockRejectedValue(new Error('boom'));
    await renderSettled();
    fireEvent.click(screen.getByTestId('vmware-install-vmware-aiops'));
    await waitFor(() => expect(screen.getByText(/boom/)).toBeTruthy());
    // Failed → back to an actionable install button, not stuck.
    expect(screen.getByTestId('vmware-install-vmware-aiops')).toBeEnabled();
  });

  it('renders the inline error state when the catalog load fails', async () => {
    mockedClient.listMarketplace.mockRejectedValue(new Error('catalog down'));
    renderStarter();
    await waitFor(() => expect(screen.getByText(/catalog down/)).toBeTruthy());
    expect(screen.queryAllByTestId(/^vmware-install-/)).toHaveLength(0);
  });

  it('prefers name_zh under zh-CN with fallback to name when missing', async () => {
    localStorage.setItem('xiaoguai.locale', 'zh-CN');
    await renderSettled();
    // name_zh present → shown.
    expect(screen.getByText('VMware 监控')).toBeTruthy();
    expect(screen.getByText('VMware 运维')).toBeTruthy();
    // name_zh absent (vmware-storage) → falls back to `name`.
    expect(screen.getByText('VMware Storage')).toBeTruthy();
  });

  it('states the honest prerequisite (register-only; uv tool install + vCenter config)', async () => {
    await renderSettled();
    expect(screen.getByText(/uv tool install/)).toBeTruthy();
  });
});
