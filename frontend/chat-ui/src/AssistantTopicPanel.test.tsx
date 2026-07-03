/**
 * AssistantTopicPanel — expert prerequisite gating (v1.34).
 *
 * Covers:
 *  - A not-ready expert persona renders LOCKED: aria-disabled, a "Install
 *    first: …" hint, and a "Set up →" CTA; clicking the name does NOT attach.
 *  - The CTA routes to /skills.
 *  - A ready expert (and an ordinary persona with no blueprint) is selectable.
 *  - A failed /v1/experts fetch never locks anything (fail-open).
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { ExpertReadinessResponse, Persona } from '@xiaoguai/shared';
import { I18nProvider } from './i18n/I18nProvider';
import { AssistantTopicPanel } from './AssistantTopicPanel';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async (orig) => {
  const actual = await orig<typeof import('react-router-dom')>();
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('./client', () => ({
  client: {
    listPersonas: vi.fn(),
    listTeams: vi.fn(),
    listExperts: vi.fn(),
    getSessionPersona: vi.fn(),
    attachSessionPersona: vi.fn(),
  },
}));
import { client } from './client';
const mc = vi.mocked(client);

function persona(id: string, name: string): Persona {
  return {
    id,
    name,
    system_prompt: 'role',
    default_model: null,
    tool_allowlist: null,
    escalation_tier: null,
    created_at: '2026-07-03T00:00:00Z',
    archived: false,
  } as Persona;
}

/** VM-ops not ready (policy group unmet); a plain persona has no blueprint. */
function experts(vmReady: boolean): ExpertReadinessResponse {
  return {
    version: 1,
    offline_hint: null,
    offline_hint_en: null,
    experts: [
      {
        key: 'vmware-ops',
        persona_name: 'VMware 运维助手',
        name: 'VMware Ops',
        name_zh: null,
        summary: null,
        summary_zh: null,
        ready: vmReady,
        required: [
          {
            label: 'Policy foundation',
            label_zh: '策略与审计基座',
            satisfied: vmReady,
            any_of: [
              { kind: 'package', id: 'vmware-policy', label: 'vmware-policy', satisfied: vmReady, install: 'uv tool install vmware-policy' },
            ],
          },
        ],
        optional: [],
      },
    ],
  };
}

function renderPanel() {
  return render(
    <MemoryRouter>
      <I18nProvider>
        <AssistantTopicPanel
          sessions={[]}
          pendingAssistant={null}
          onSelectAssistant={vi.fn()}
        />
      </I18nProvider>
    </MemoryRouter>,
  );
}

/** Open the 助手 (Assistants) tab and wait for personas to render. */
async function openAssistants() {
  const tabs = screen.getAllByRole('tab');
  fireEvent.click(tabs[0]!); // 助手 tab is first
  await waitFor(() => expect(screen.getByText('VMware 运维助手')).toBeTruthy());
}

beforeEach(() => {
  vi.clearAllMocks();
  mc.listPersonas.mockResolvedValue([
    persona('p-vm', 'VMware 运维助手'),
    persona('p-plain', '普通助手'),
  ]);
  mc.listTeams.mockResolvedValue([]);
  mc.getSessionPersona.mockResolvedValue(null);
  mc.attachSessionPersona.mockResolvedValue(undefined as never);
});

describe('AssistantTopicPanel expert gating', () => {
  it('locks a not-ready expert: disabled, hint, CTA — and clicking does not attach', async () => {
    mc.listExperts.mockResolvedValue(experts(false));
    renderPanel();
    await openAssistants();

    const vmName = screen.getByText('VMware 运维助手');
    const row = vmName.closest('.assistant-row') as HTMLElement;
    // The main button is aria-disabled and shows the unmet requirement.
    const main = row.querySelector('.assistant-row__main') as HTMLElement;
    expect(main.getAttribute('aria-disabled')).toBe('true');
    expect(within(row).getByText(/策略与审计基座|Policy foundation/)).toBeTruthy();

    // Clicking the locked name must NOT attach.
    fireEvent.click(main);
    await new Promise((r) => setTimeout(r, 0));
    expect(mc.attachSessionPersona).not.toHaveBeenCalled();

    // The CTA routes to the Skills page.
    const cta = within(row).getByText(/Set up|去安装/);
    fireEvent.click(cta);
    expect(navigateMock).toHaveBeenCalledWith('/skills');
  });

  it('a ready expert and an ordinary persona are selectable', async () => {
    mc.listExperts.mockResolvedValue(experts(true));
    renderPanel();
    await openAssistants();
    // Ordinary persona (no blueprint) → not locked.
    const plain = screen.getByText('普通助手').closest('.assistant-row') as HTMLElement;
    expect(plain.classList.contains('locked')).toBe(false);
    // Ready expert → not locked.
    const vm = screen.getByText('VMware 运维助手').closest('.assistant-row') as HTMLElement;
    expect(vm.classList.contains('locked')).toBe(false);
  });

  it('fails open: a broken /v1/experts never locks a persona', async () => {
    mc.listExperts.mockRejectedValue(new Error('down'));
    renderPanel();
    await openAssistants();
    const vm = screen.getByText('VMware 运维助手').closest('.assistant-row') as HTMLElement;
    expect(vm.classList.contains('locked')).toBe(false);
  });
});
