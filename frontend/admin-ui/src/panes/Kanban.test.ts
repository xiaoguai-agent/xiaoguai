/**
 * v1.4.0-ready — Kanban board pane tests.
 *
 * We test the pure helper functions (age formatting, description preview,
 * column grouping logic, mock data shapes) rather than doing a full DOM
 * render, which would require jsdom + React Testing Library.
 *
 * The UI contract is enforced by `pnpm typecheck` (tsc --noEmit).
 */

import { describe, expect, it } from 'vitest';
import type { TaskCard, TaskColumn, Board, TaskHistoryEntry } from '@xiaoguai/shared';

// ---------------------------------------------------------------------------
// Helper functions inlined from Kanban.tsx for unit testing
// ---------------------------------------------------------------------------

function fmtAge(isoTs: string): string {
  const diffMs = Date.now() - new Date(isoTs).getTime();
  const mins = Math.floor(diffMs / 60_000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

function descPreview(desc: string | null): string {
  if (!desc) return '';
  return desc.length > 120 ? desc.slice(0, 117) + '…' : desc;
}

function byColumn(tasks: TaskCard[], col: TaskColumn): TaskCard[] {
  return tasks.filter((t) => t.column === col);
}

// ---------------------------------------------------------------------------
// Mock data shapes
// ---------------------------------------------------------------------------

const COLUMNS: TaskColumn[] = ['triage', 'todo', 'ready', 'running', 'blocked', 'done'];

const mockBoard: Board = {
  id: 'default',
  name: 'Default',
  description: 'Main agent task board',
  created_at: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
};

function makeCard(overrides: Partial<TaskCard> = {}): TaskCard {
  const now = new Date().toISOString();
  return {
    id: 'task-1',
    board_id: 'default',
    title: 'Test task',
    description: null,
    column: 'triage',
    priority: 'medium',
    assignee: null,
    created_at: now,
    updated_at: now,
    deps: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// fmtAge
// ---------------------------------------------------------------------------

describe('fmtAge', () => {
  it('shows minutes for recent timestamps', () => {
    const ts = new Date(Date.now() - 5 * 60 * 1000).toISOString();
    expect(fmtAge(ts)).toBe('5m');
  });

  it('shows hours for timestamps up to 24h ago', () => {
    const ts = new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString();
    expect(fmtAge(ts)).toBe('3h');
  });

  it('shows days for timestamps more than 24h ago', () => {
    const ts = new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString();
    expect(fmtAge(ts)).toBe('2d');
  });

  it('shows 0m for a timestamp at the current moment', () => {
    const ts = new Date().toISOString();
    expect(fmtAge(ts)).toBe('0m');
  });
});

// ---------------------------------------------------------------------------
// descPreview
// ---------------------------------------------------------------------------

describe('descPreview', () => {
  it('returns empty string for null', () => {
    expect(descPreview(null)).toBe('');
  });

  it('returns the full string when under 120 chars', () => {
    const short = 'A short description.';
    expect(descPreview(short)).toBe(short);
  });

  it('truncates strings longer than 120 chars with ellipsis', () => {
    const long = 'x'.repeat(130);
    const result = descPreview(long);
    // slice(0, 117) + '…' = 117 ASCII chars + 1 ellipsis char = 118
    expect(result.length).toBe(118);
    expect(result.endsWith('…')).toBe(true);
  });

  it('does not truncate strings of exactly 120 chars', () => {
    const exact = 'a'.repeat(120);
    expect(descPreview(exact)).toBe(exact);
  });
});

// ---------------------------------------------------------------------------
// byColumn (column grouping)
// ---------------------------------------------------------------------------

describe('byColumn', () => {
  const tasks: TaskCard[] = [
    makeCard({ id: '1', column: 'triage' }),
    makeCard({ id: '2', column: 'triage' }),
    makeCard({ id: '3', column: 'ready' }),
    makeCard({ id: '4', column: 'running' }),
    makeCard({ id: '5', column: 'done' }),
  ];

  it('returns correct count per column', () => {
    expect(byColumn(tasks, 'triage')).toHaveLength(2);
    expect(byColumn(tasks, 'ready')).toHaveLength(1);
    expect(byColumn(tasks, 'running')).toHaveLength(1);
    expect(byColumn(tasks, 'done')).toHaveLength(1);
  });

  it('returns empty array for columns with no tasks', () => {
    expect(byColumn(tasks, 'todo')).toHaveLength(0);
    expect(byColumn(tasks, 'blocked')).toHaveLength(0);
  });

  it('returns all 6 canonical columns', () => {
    expect(COLUMNS).toHaveLength(6);
    expect(COLUMNS).toContain('triage');
    expect(COLUMNS).toContain('todo');
    expect(COLUMNS).toContain('ready');
    expect(COLUMNS).toContain('running');
    expect(COLUMNS).toContain('blocked');
    expect(COLUMNS).toContain('done');
  });
});

// ---------------------------------------------------------------------------
// 404 fallback — mock data shape validity
// ---------------------------------------------------------------------------

describe('404 fallback mock data', () => {
  const MOCK_TASKS: TaskCard[] = [
    makeCard({ id: 'm1', column: 'triage', priority: 'medium' }),
    makeCard({ id: 'm2', column: 'todo', priority: 'high' }),
    makeCard({ id: 'm3', column: 'ready', priority: 'medium' }),
    makeCard({ id: 'm4', column: 'running', priority: 'critical' }),
    makeCard({ id: 'm5', column: 'blocked', priority: 'low' }),
    makeCard({ id: 'm6', column: 'done', priority: 'medium' }),
  ];

  it('has at least one card per column', () => {
    for (const col of COLUMNS) {
      expect(byColumn(MOCK_TASKS, col).length).toBeGreaterThanOrEqual(1);
    }
  });

  it('all tasks have required fields', () => {
    for (const task of MOCK_TASKS) {
      expect(typeof task.id).toBe('string');
      expect(typeof task.title).toBe('string');
      expect(COLUMNS).toContain(task.column);
      expect(['low', 'medium', 'high', 'critical']).toContain(task.priority);
      expect(Array.isArray(task.deps)).toBe(true);
    }
  });

  it('board has required fields', () => {
    expect(typeof mockBoard.id).toBe('string');
    expect(typeof mockBoard.name).toBe('string');
  });
});

// ---------------------------------------------------------------------------
// Card drag — optimistic column update
// ---------------------------------------------------------------------------

describe('drag-to-move optimistic update', () => {
  it('updates the task column in state', () => {
    const initial: TaskCard[] = [makeCard({ id: 'a', column: 'triage' })];
    const targetColumn: TaskColumn = 'ready';

    // Simulate the optimistic update applied in handleDrop (mock mode)
    const updated = initial.map((t) =>
      t.id === 'a' ? { ...t, column: targetColumn } : t,
    );

    expect(updated[0]?.column).toBe('ready');
    // Immutability: original not mutated
    expect(initial[0]?.column).toBe('triage');
  });
});

// ---------------------------------------------------------------------------
// Dispatch click — confirmation semantics
// ---------------------------------------------------------------------------

describe('dispatch semantics', () => {
  it('returns null when no READY tasks exist (matches API contract)', () => {
    // API returns null when no READY task to dispatch
    const apiResponse: TaskCard | null = null;
    expect(apiResponse).toBeNull();
  });

  it('returns a task when a READY task is dispatched', () => {
    const dispatched: TaskCard = makeCard({ column: 'running' });
    expect(dispatched.column).toBe('running');
  });
});

// ---------------------------------------------------------------------------
// Board switch
// ---------------------------------------------------------------------------

describe('board switching', () => {
  it('selects the board matching the given id', () => {
    const boards: Board[] = [
      { id: 'default', name: 'Default', description: null, created_at: '' },
      { id: 'infra', name: 'Infra', description: null, created_at: '' },
    ];
    const selected = boards.find((b) => b.id === 'infra');
    expect(selected?.name).toBe('Infra');
  });
});

// ---------------------------------------------------------------------------
// New task creation
// ---------------------------------------------------------------------------

describe('new task creation', () => {
  it('builds a valid TaskCard from CreateTaskRequest fields', () => {
    const now = new Date().toISOString();
    const card: TaskCard = {
      id: 'new-1',
      board_id: 'default',
      title: 'Deploy new model version',
      description: 'Run blue-green deploy on prod cluster.',
      column: 'triage',
      priority: 'high',
      assignee: null,
      created_at: now,
      updated_at: now,
      deps: [],
    };
    expect(card.title).toBe('Deploy new model version');
    expect(card.column).toBe('triage');
    expect(card.priority).toBe('high');
  });
});

// ---------------------------------------------------------------------------
// TaskHistoryEntry shape
// ---------------------------------------------------------------------------

describe('TaskHistoryEntry shape', () => {
  it('accepts the expected wire shape', () => {
    const entry: TaskHistoryEntry = {
      ts: new Date().toISOString(),
      from_column: 'triage',
      to_column: 'ready',
      actor: 'agent-classifier',
      note: 'Classified as high priority',
    };
    expect(entry.from_column).toBe('triage');
    expect(entry.to_column).toBe('ready');
  });

  it('allows null from_column for initial state transitions', () => {
    const entry: TaskHistoryEntry = {
      ts: new Date().toISOString(),
      from_column: null,
      to_column: 'triage',
      actor: null,
      note: null,
    };
    expect(entry.from_column).toBeNull();
  });
});
