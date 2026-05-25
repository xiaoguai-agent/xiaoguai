/**
 * v1.4.0-ready — Kanban board page (Hermes-inspired).
 *
 * Layout:
 *   - Top toolbar: board switcher, New board, Refresh, Dispatch
 *   - Six columns side-by-side: TRIAGE / TO-DO / READY / RUNNING / BLOCKED / DONE
 *   - Cards: title, 2-line description preview, assignee badge, priority chip, age
 *   - Card click → right-side detail drawer
 *   - New task modal
 *   - HTML5 drag-and-drop to move cards between columns
 *
 * 404 fallback: if /v1/tasks/* returns 404 (kanban not yet in backend),
 * shows an informative banner and renders MOCK demo data so the board
 * is browsable for demo/screenshot purposes.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  Board,
  BlockTaskRequest,
  CreateTaskRequest,
  TaskCard,
  TaskColumn,
  TaskHistoryEntry,
  TaskPriority,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';
import { client } from '../client';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const COLUMNS: { key: TaskColumn; label: string }[] = [
  { key: 'triage', label: 'TRIAGE' },
  { key: 'todo', label: 'TO-DO' },
  { key: 'ready', label: 'READY' },
  { key: 'running', label: 'RUNNING' },
  { key: 'blocked', label: 'BLOCKED' },
  { key: 'done', label: 'DONE' },
];

const PRIORITY_COLORS: Record<TaskPriority, string> = {
  low: '#64748b',
  medium: '#f59e0b',
  high: '#ef4444',
  critical: '#7c3aed',
};

const PRIORITY_BG: Record<TaskPriority, string> = {
  low: '#f1f5f9',
  medium: '#fef3c7',
  high: '#fee2e2',
  critical: '#ede9fe',
};

const COLUMN_COLORS: Record<TaskColumn, string> = {
  triage: '#6b7280',
  todo: '#3b82f6',
  ready: '#10b981',
  running: '#f59e0b',
  blocked: '#ef4444',
  done: '#8b5cf6',
};

const DEFAULT_BOARD_ID = 'default';

// ---------------------------------------------------------------------------
// Mock data — shown when backend returns 404
// ---------------------------------------------------------------------------

const MOCK_TASKS: TaskCard[] = [
  {
    id: 'mock-1',
    board_id: DEFAULT_BOARD_ID,
    title: 'Ingest new customer feedback batch',
    description: 'Pull Q2 survey responses from Typeform and classify by sentiment and category.',
    column: 'triage',
    priority: 'medium',
    assignee: 'agent-classifier',
    created_at: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString(),
    deps: [],
  },
  {
    id: 'mock-2',
    board_id: DEFAULT_BOARD_ID,
    title: 'Generate weekly revenue report',
    description: 'Aggregate sales data from Stripe and produce a Markdown summary for the exec team.',
    column: 'todo',
    priority: 'high',
    assignee: 'agent-finance',
    created_at: new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 1 * 24 * 60 * 60 * 1000).toISOString(),
    deps: [],
  },
  {
    id: 'mock-3',
    board_id: DEFAULT_BOARD_ID,
    title: 'Draft release notes for v1.4',
    description: 'Summarize all merged PRs since v1.3 and write user-facing changelog.',
    column: 'ready',
    priority: 'medium',
    assignee: 'agent-writer',
    created_at: new Date(Date.now() - 2 * 24 * 60 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 5 * 60 * 60 * 1000).toISOString(),
    deps: [],
  },
  {
    id: 'mock-4',
    board_id: DEFAULT_BOARD_ID,
    title: 'Scan security advisories',
    description: 'Check NVD feeds for CVEs affecting our dependency tree and open issues as needed.',
    column: 'running',
    priority: 'critical',
    assignee: 'agent-security',
    created_at: new Date(Date.now() - 30 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 5 * 60 * 1000).toISOString(),
    deps: [],
  },
  {
    id: 'mock-5',
    board_id: DEFAULT_BOARD_ID,
    title: 'Update partner API docs',
    description: 'Waiting for Legal to sign off on new data-sharing terms before publishing.',
    column: 'blocked',
    priority: 'low',
    assignee: 'agent-writer',
    created_at: new Date(Date.now() - 3 * 24 * 60 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 3 * 24 * 60 * 60 * 1000).toISOString(),
    deps: [],
  },
  {
    id: 'mock-6',
    board_id: DEFAULT_BOARD_ID,
    title: 'Monthly backup verification',
    description: 'Restore a random sample from last month\'s snapshot and verify integrity.',
    column: 'done',
    priority: 'medium',
    assignee: 'agent-ops',
    created_at: new Date(Date.now() - 5 * 24 * 60 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 4 * 60 * 60 * 1000).toISOString(),
    deps: [],
  },
  {
    id: 'mock-7',
    board_id: DEFAULT_BOARD_ID,
    title: 'Translate UI strings to Japanese',
    description: 'Run i18n extraction, send to translation memory, import back.',
    column: 'todo',
    priority: 'low',
    assignee: null,
    created_at: new Date(Date.now() - 6 * 60 * 60 * 1000).toISOString(),
    updated_at: new Date(Date.now() - 6 * 60 * 60 * 1000).toISOString(),
    deps: [],
  },
];

const MOCK_BOARDS: Board[] = [
  {
    id: DEFAULT_BOARD_ID,
    name: 'Default',
    description: 'Main agent task board',
    created_at: new Date(Date.now() - 30 * 24 * 60 * 60 * 1000).toISOString(),
  },
];

// ---------------------------------------------------------------------------
// Helpers
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

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

interface PriorityChipProps {
  priority: TaskPriority;
}

function PriorityChip({ priority }: PriorityChipProps): JSX.Element {
  return (
    <span
      style={{
        display: 'inline-block',
        padding: '1px 6px',
        borderRadius: 4,
        fontSize: 10,
        fontWeight: 700,
        letterSpacing: '0.05em',
        textTransform: 'uppercase',
        background: PRIORITY_BG[priority],
        color: PRIORITY_COLORS[priority],
      }}
    >
      {priority}
    </span>
  );
}

interface AssigneeBadgeProps {
  assignee: string | null;
}

function AssigneeBadge({ assignee }: AssigneeBadgeProps): JSX.Element | null {
  if (!assignee) return null;
  const initials = assignee.replace('agent-', '').slice(0, 2).toUpperCase();
  return (
    <span
      title={assignee}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        width: 22,
        height: 22,
        borderRadius: '50%',
        background: '#e0e7ff',
        color: '#4338ca',
        fontSize: 9,
        fontWeight: 700,
        flexShrink: 0,
      }}
    >
      {initials}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Card component (with HTML5 drag source)
// ---------------------------------------------------------------------------

interface KanbanCardProps {
  card: TaskCard;
  onClick: (card: TaskCard) => void;
  onDragStart: (card: TaskCard) => void;
}

function KanbanCard({ card, onClick, onDragStart }: KanbanCardProps): JSX.Element {
  return (
    <div
      draggable
      onDragStart={() => onDragStart(card)}
      onClick={() => onClick(card)}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') onClick(card);
      }}
      aria-label={`Task: ${card.title}`}
      style={{
        background: '#ffffff',
        border: '1px solid #e2e8f0',
        borderRadius: 8,
        padding: '10px 12px',
        marginBottom: 8,
        cursor: 'grab',
        transition: 'box-shadow 120ms ease, border-color 120ms ease',
      }}
      onMouseEnter={(e) => {
        (e.currentTarget as HTMLDivElement).style.boxShadow =
          '0 2px 8px rgba(15,23,42,0.08)';
        (e.currentTarget as HTMLDivElement).style.borderColor = '#cbd5e1';
      }}
      onMouseLeave={(e) => {
        (e.currentTarget as HTMLDivElement).style.boxShadow = 'none';
        (e.currentTarget as HTMLDivElement).style.borderColor = '#e2e8f0';
      }}
    >
      {/* Title */}
      <div
        style={{
          fontSize: 13,
          fontWeight: 500,
          color: '#0f172a',
          marginBottom: 4,
          lineHeight: 1.4,
        }}
      >
        {card.title}
      </div>

      {/* Description preview */}
      {card.description && (
        <div
          style={{
            fontSize: 12,
            color: '#64748b',
            lineHeight: 1.45,
            marginBottom: 8,
            overflow: 'hidden',
            display: '-webkit-box',
            WebkitLineClamp: 2,
            WebkitBoxOrient: 'vertical',
          }}
        >
          {descPreview(card.description)}
        </div>
      )}

      {/* Footer row */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          flexWrap: 'wrap',
        }}
      >
        <PriorityChip priority={card.priority} />
        <AssigneeBadge assignee={card.assignee} />
        <span
          style={{
            marginLeft: 'auto',
            fontSize: 11,
            color: '#94a3b8',
            whiteSpace: 'nowrap',
          }}
        >
          {fmtAge(card.created_at)}
        </span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Column component (with HTML5 drop target)
// ---------------------------------------------------------------------------

interface KanbanColumnProps {
  column: TaskColumn;
  label: string;
  cards: TaskCard[];
  onCardClick: (card: TaskCard) => void;
  onDragStart: (card: TaskCard) => void;
  onDrop: (column: TaskColumn) => void;
}

function KanbanColumn({
  column,
  label,
  cards,
  onCardClick,
  onDragStart,
  onDrop,
}: KanbanColumnProps): JSX.Element {
  const [dragOver, setDragOver] = useState(false);
  const color = COLUMN_COLORS[column];

  return (
    <div
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        onDrop(column);
      }}
      style={{
        flex: '1 1 0',
        minWidth: 0,
        background: dragOver ? '#f0f9ff' : '#f8fafc',
        border: dragOver ? `2px dashed ${color}` : '2px dashed transparent',
        borderRadius: 10,
        padding: '0 8px 8px',
        transition: 'background 120ms ease, border-color 120ms ease',
      }}
    >
      {/* Column header */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: '10px 4px 8px',
          marginBottom: 4,
          borderBottom: `2px solid ${color}`,
        }}
      >
        <span
          style={{
            fontSize: 11,
            fontWeight: 700,
            letterSpacing: '0.08em',
            color,
          }}
        >
          {label}
        </span>
        <span
          style={{
            marginLeft: 'auto',
            fontSize: 11,
            fontWeight: 600,
            background: '#e2e8f0',
            color: '#64748b',
            borderRadius: 999,
            padding: '0 6px',
            minWidth: 18,
            textAlign: 'center',
          }}
        >
          {cards.length}
        </span>
      </div>

      {/* Cards */}
      {cards.map((card) => (
        <KanbanCard
          key={card.id}
          card={card}
          onClick={onCardClick}
          onDragStart={onDragStart}
        />
      ))}

      {/* Empty state */}
      {cards.length === 0 && (
        <div
          style={{
            textAlign: 'center',
            padding: '24px 8px',
            color: '#94a3b8',
            fontSize: 13,
          }}
        >
          —
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Detail drawer
// ---------------------------------------------------------------------------

interface DrawerProps {
  card: TaskCard;
  history: TaskHistoryEntry[];
  historyLoading: boolean;
  isMock: boolean;
  onClose: () => void;
  onMoveToColumn: (col: TaskColumn) => void;
  onBlock: (reason: string) => void;
}

function TaskDrawer({
  card,
  history,
  historyLoading,
  isMock,
  onClose,
  onMoveToColumn,
  onBlock,
}: DrawerProps): JSX.Element {
  const [blockMode, setBlockMode] = useState(false);
  const [blockReason, setBlockReason] = useState('');
  const [moveTarget, setMoveTarget] = useState<TaskColumn>(card.column);

  return (
    <div
      className="drawer-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      role="dialog"
      aria-modal="true"
      aria-label={`Task detail: ${card.title}`}
    >
      <div className="drawer">
        <div className="drawer-header">
          <h2>{card.title}</h2>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>

        <dl className="drawer-grid">
          <dt>Column</dt>
          <dd>
            <span
              style={{
                display: 'inline-block',
                padding: '1px 8px',
                borderRadius: 4,
                fontSize: 11,
                fontWeight: 600,
                background: PRIORITY_BG['medium'],
                color: COLUMN_COLORS[card.column],
              }}
            >
              {card.column.toUpperCase()}
            </span>
          </dd>
          <dt>Priority</dt>
          <dd>
            <PriorityChip priority={card.priority} />
          </dd>
          <dt>Assignee</dt>
          <dd className="muted">{card.assignee ?? '(unassigned)'}</dd>
          <dt>Created</dt>
          <dd className="muted">{new Date(card.created_at).toLocaleString()}</dd>
          <dt>Updated</dt>
          <dd className="muted">{new Date(card.updated_at).toLocaleString()}</dd>
          {card.deps.length > 0 && (
            <>
              <dt>Deps</dt>
              <dd className="muted">{card.deps.join(', ')}</dd>
            </>
          )}
        </dl>

        {card.description && (
          <div style={{ marginTop: 16 }}>
            <div
              style={{
                fontSize: 12,
                fontWeight: 600,
                color: '#64748b',
                textTransform: 'uppercase',
                letterSpacing: '0.04em',
                marginBottom: 6,
              }}
            >
              Description
            </div>
            <p style={{ margin: 0, fontSize: 13, lineHeight: 1.55, color: '#0f172a' }}>
              {card.description}
            </p>
          </div>
        )}

        {/* Move to */}
        <div style={{ marginTop: 20 }}>
          <div
            style={{
              fontSize: 12,
              fontWeight: 600,
              color: '#64748b',
              textTransform: 'uppercase',
              letterSpacing: '0.04em',
              marginBottom: 8,
            }}
          >
            Move to
          </div>
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', alignItems: 'center' }}>
            <select
              value={moveTarget}
              onChange={(e) => setMoveTarget(e.target.value as TaskColumn)}
              style={{
                font: 'inherit',
                fontSize: 13,
                padding: '5px 10px',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
                background: '#ffffff',
              }}
              disabled={isMock}
            >
              {COLUMNS.map((c) => (
                <option key={c.key} value={c.key}>
                  {c.label}
                </option>
              ))}
            </select>
            <button
              className="run-btn"
              style={{ padding: '6px 14px', fontSize: 13 }}
              disabled={moveTarget === card.column || isMock}
              onClick={() => onMoveToColumn(moveTarget)}
            >
              Move
            </button>
          </div>
          {isMock && (
            <div
              style={{ fontSize: 11, color: '#94a3b8', marginTop: 4 }}
            >
              Move disabled in mock mode.
            </div>
          )}
        </div>

        {/* Block action */}
        {!blockMode ? (
          <div style={{ marginTop: 12 }}>
            <button
              style={{
                font: 'inherit',
                fontSize: 12,
                padding: '4px 12px',
                border: '1px solid #fecaca',
                background: '#fef2f2',
                color: '#b91c1c',
                borderRadius: 6,
                cursor: isMock ? 'not-allowed' : 'pointer',
                opacity: isMock ? 0.6 : 1,
              }}
              disabled={isMock}
              onClick={() => setBlockMode(true)}
            >
              Block (with reason)
            </button>
          </div>
        ) : (
          <div style={{ marginTop: 12 }}>
            <textarea
              autoFocus
              value={blockReason}
              onChange={(e) => setBlockReason(e.target.value)}
              placeholder="Reason for blocking…"
              style={{
                display: 'block',
                width: '100%',
                font: 'inherit',
                fontSize: 13,
                padding: '8px 10px',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
                resize: 'vertical',
                minHeight: 72,
                marginBottom: 8,
              }}
            />
            <div style={{ display: 'flex', gap: 8 }}>
              <button
                className="run-btn"
                style={{ padding: '6px 14px', fontSize: 13 }}
                disabled={!blockReason.trim()}
                onClick={() => {
                  onBlock(blockReason.trim());
                  setBlockMode(false);
                  setBlockReason('');
                }}
              >
                Confirm block
              </button>
              <button
                style={{
                  font: 'inherit',
                  fontSize: 12,
                  padding: '6px 14px',
                  border: '1px solid #e2e8f0',
                  background: '#fff',
                  borderRadius: 6,
                  cursor: 'pointer',
                }}
                onClick={() => setBlockMode(false)}
              >
                Cancel
              </button>
            </div>
          </div>
        )}

        {/* History */}
        <div style={{ marginTop: 24 }}>
          <div
            style={{
              fontSize: 12,
              fontWeight: 600,
              color: '#64748b',
              textTransform: 'uppercase',
              letterSpacing: '0.04em',
              marginBottom: 8,
            }}
          >
            History
          </div>
          {historyLoading ? (
            <div style={{ color: '#94a3b8', fontSize: 13 }}>Loading…</div>
          ) : history.length === 0 ? (
            <div style={{ color: '#94a3b8', fontSize: 13 }}>No transitions yet.</div>
          ) : (
            <ul style={{ margin: 0, padding: 0, listStyle: 'none' }}>
              {history.map((h, i) => (
                <li
                  key={i}
                  style={{
                    fontSize: 12,
                    color: '#475569',
                    padding: '4px 0',
                    borderBottom: '1px solid #f1f5f9',
                    display: 'flex',
                    gap: 8,
                    flexWrap: 'wrap',
                  }}
                >
                  <span style={{ color: '#94a3b8', whiteSpace: 'nowrap' }}>
                    {new Date(h.ts).toLocaleTimeString()}
                  </span>
                  <span>
                    {h.from_column ? `${h.from_column} → ` : ''}
                    <strong>{h.to_column}</strong>
                    {h.actor ? ` by ${h.actor}` : ''}
                    {h.note ? ` — ${h.note}` : ''}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// New task modal
// ---------------------------------------------------------------------------

interface NewTaskModalProps {
  boards: Board[];
  currentBoardId: string;
  isMock: boolean;
  onClose: () => void;
  onCreate: (req: CreateTaskRequest) => void;
}

function NewTaskModal({
  boards,
  currentBoardId,
  isMock,
  onClose,
  onCreate,
}: NewTaskModalProps): JSX.Element {
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [column, setColumn] = useState<TaskColumn>('triage');
  const [priority, setPriority] = useState<TaskPriority>('medium');
  const [boardId, setBoardId] = useState(currentBoardId);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!title.trim()) return;
    onCreate({ board_id: boardId, title: title.trim(), description: description || null, column, priority });
    onClose();
  };

  return (
    <div
      className="drawer-backdrop"
      style={{ justifyContent: 'center', alignItems: 'center' }}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      role="dialog"
      aria-modal="true"
      aria-label="New task"
    >
      <div
        style={{
          background: '#ffffff',
          borderRadius: 12,
          padding: '24px 28px',
          width: 'min(480px, 92vw)',
          boxShadow: '0 8px 32px rgba(15,23,42,0.15)',
        }}
      >
        <div className="drawer-header">
          <h2>New Task</h2>
          <button className="drawer-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </div>
        {isMock && (
          <div
            style={{
              background: '#fef3c7',
              color: '#92400e',
              border: '1px solid #fde68a',
              borderRadius: 6,
              padding: '8px 12px',
              fontSize: 12,
              marginBottom: 14,
            }}
          >
            Preview mode — task will be added to the mock board locally only.
          </div>
        )}
        <form onSubmit={handleSubmit} style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
            Title <span style={{ color: '#ef4444' }}>*</span>
            <input
              autoFocus
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="Task title"
              required
              style={{
                font: 'inherit',
                fontSize: 13,
                padding: '7px 10px',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
              }}
            />
          </label>
          <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
            Description
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              placeholder="Optional description…"
              style={{
                font: 'inherit',
                fontSize: 13,
                padding: '7px 10px',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
                resize: 'vertical',
                minHeight: 72,
              }}
            />
          </label>
          <div style={{ display: 'flex', gap: 12 }}>
            <label style={{ flex: 1, display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
              Column
              <select
                value={column}
                onChange={(e) => setColumn(e.target.value as TaskColumn)}
                style={{
                  font: 'inherit',
                  fontSize: 13,
                  padding: '7px 10px',
                  border: '1px solid #e2e8f0',
                  borderRadius: 6,
                }}
              >
                {COLUMNS.map((c) => (
                  <option key={c.key} value={c.key}>
                    {c.label}
                  </option>
                ))}
              </select>
            </label>
            <label style={{ flex: 1, display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
              Priority
              <select
                value={priority}
                onChange={(e) => setPriority(e.target.value as TaskPriority)}
                style={{
                  font: 'inherit',
                  fontSize: 13,
                  padding: '7px 10px',
                  border: '1px solid #e2e8f0',
                  borderRadius: 6,
                }}
              >
                {(['low', 'medium', 'high', 'critical'] as TaskPriority[]).map((p) => (
                  <option key={p} value={p}>
                    {p}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <label style={{ display: 'flex', flexDirection: 'column', gap: 4, fontSize: 13 }}>
            Board
            <select
              value={boardId}
              onChange={(e) => setBoardId(e.target.value)}
              style={{
                font: 'inherit',
                fontSize: 13,
                padding: '7px 10px',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
              }}
            >
              {boards.map((b) => (
                <option key={b.id} value={b.id}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 4 }}>
            <button
              type="button"
              onClick={onClose}
              style={{
                font: 'inherit',
                fontSize: 13,
                padding: '7px 16px',
                border: '1px solid #e2e8f0',
                background: '#fff',
                borderRadius: 6,
                cursor: 'pointer',
              }}
            >
              Cancel
            </button>
            <button
              type="submit"
              className="run-btn"
              disabled={!title.trim()}
              style={{ padding: '7px 20px', fontSize: 13 }}
            >
              Create
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main Kanban pane
// ---------------------------------------------------------------------------

export function KanbanPane(): JSX.Element {
  const [boards, setBoards] = useState<Board[]>([]);
  const [activeBoardId, setActiveBoardId] = useState<string>(DEFAULT_BOARD_ID);
  const [tasks, setTasks] = useState<TaskCard[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isMock, setIsMock] = useState(false);
  const [selectedCard, setSelectedCard] = useState<TaskCard | null>(null);
  const [cardHistory, setCardHistory] = useState<TaskHistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [showNewTask, setShowNewTask] = useState(false);
  const [dispatching, setDispatching] = useState(false);
  const [dispatchMsg, setDispatchMsg] = useState<string | null>(null);
  const dragCardRef = useRef<TaskCard | null>(null);

  // Load boards — with 404 fallback
  const loadBoards = useCallback(async () => {
    try {
      const rows = await client.listBoards();
      setBoards(rows.length > 0 ? rows : MOCK_BOARDS);
      if (rows.length === 0) setIsMock(true);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        setBoards(MOCK_BOARDS);
        setIsMock(true);
      }
      // silently fall through — tasks fetch will also 404 and set isMock
    }
  }, []);

  // Load tasks — with 404 fallback
  const loadTasks = useCallback(async (boardId: string) => {
    setLoading(true);
    setError(null);
    try {
      const rows = await client.listTasks({ board_id: boardId });
      setTasks(rows);
      setIsMock(false);
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        setTasks(MOCK_TASKS);
        setIsMock(true);
        setBoards(MOCK_BOARDS);
      } else {
        setError((e as Error).message);
        setTasks([]);
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadBoards();
  }, [loadBoards]);

  useEffect(() => {
    void loadTasks(activeBoardId);
  }, [activeBoardId, loadTasks]);

  const handleRefresh = () => void loadTasks(activeBoardId);

  const handleDispatch = async () => {
    if (isMock) {
      setDispatchMsg('Dispatch disabled in mock mode.');
      setTimeout(() => setDispatchMsg(null), 3000);
      return;
    }
    setDispatching(true);
    try {
      const moved = await client.dispatchTask(activeBoardId);
      if (moved) {
        setDispatchMsg(`Dispatched: "${moved.title}"`);
        await loadTasks(activeBoardId);
      } else {
        setDispatchMsg('No READY tasks to dispatch.');
      }
    } catch (e) {
      setDispatchMsg(`Dispatch failed: ${(e as Error).message}`);
    } finally {
      setDispatching(false);
      setTimeout(() => setDispatchMsg(null), 4000);
    }
  };

  const handleCardClick = async (card: TaskCard) => {
    setSelectedCard(card);
    setCardHistory([]);
    if (!isMock) {
      setHistoryLoading(true);
      try {
        const h = await client.getTaskHistory(card.id);
        setCardHistory(h);
      } catch {
        setCardHistory([]);
      } finally {
        setHistoryLoading(false);
      }
    }
  };

  const handleMoveToColumn = async (col: TaskColumn) => {
    if (!selectedCard || isMock) return;
    try {
      const updated = await client.updateTaskColumn(selectedCard.id, { column: col });
      setTasks((prev) => prev.map((t) => (t.id === updated.id ? updated : t)));
      setSelectedCard(updated);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const handleBlock = async (reason: string) => {
    if (!selectedCard || isMock) return;
    try {
      const updated = await client.blockTask(selectedCard.id, { reason } as BlockTaskRequest);
      setTasks((prev) => prev.map((t) => (t.id === updated.id ? updated : t)));
      setSelectedCard(updated);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const handleDragStart = (card: TaskCard) => {
    dragCardRef.current = card;
  };

  const handleDrop = async (targetColumn: TaskColumn) => {
    const card = dragCardRef.current;
    dragCardRef.current = null;
    if (!card || card.column === targetColumn) return;

    if (isMock) {
      // Optimistic update in mock mode
      setTasks((prev) =>
        prev.map((t) => (t.id === card.id ? { ...t, column: targetColumn } : t)),
      );
      return;
    }

    try {
      const updated = await client.updateTaskColumn(card.id, { column: targetColumn });
      setTasks((prev) => prev.map((t) => (t.id === updated.id ? updated : t)));
    } catch (e) {
      setError((e as Error).message);
      await loadTasks(activeBoardId);
    }
  };

  const handleCreateTask = async (req: CreateTaskRequest) => {
    if (isMock) {
      // Add locally with a fake id
      const now = new Date().toISOString();
      const fakeCard: TaskCard = {
        id: `mock-${Date.now()}`,
        board_id: req.board_id,
        title: req.title,
        description: req.description ?? null,
        column: req.column ?? 'triage',
        priority: req.priority ?? 'medium',
        assignee: req.assignee ?? null,
        created_at: now,
        updated_at: now,
        deps: [],
      };
      setTasks((prev) => [...prev, fakeCard]);
      return;
    }
    try {
      const card = await client.createTask(req);
      setTasks((prev) => [...prev, card]);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  // Group tasks by column
  const byColumn = (col: TaskColumn) => tasks.filter((t) => t.column === col);

  return (
    <>
      {/* Header */}
      <header className="today-header">
        <h1>Kanban</h1>
        <div className="today-meta">
          {/* Board switcher */}
          <label style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13 }}>
            Board
            <select
              value={activeBoardId}
              onChange={(e) => setActiveBoardId(e.target.value)}
              style={{
                font: 'inherit',
                fontSize: 13,
                padding: '4px 10px',
                border: '1px solid #e2e8f0',
                borderRadius: 6,
                background: '#fff',
              }}
            >
              {boards.map((b) => (
                <option key={b.id} value={b.id}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>

          <button
            onClick={() => setShowNewTask(true)}
            style={{
              padding: '4px 12px',
              border: '1px solid #e2e8f0',
              background: '#ffffff',
              borderRadius: 6,
              cursor: 'pointer',
              font: 'inherit',
              fontSize: 12,
            }}
          >
            + New task
          </button>

          <button
            onClick={handleRefresh}
            disabled={loading}
            style={{
              padding: '4px 12px',
              border: '1px solid #e2e8f0',
              background: '#ffffff',
              borderRadius: 6,
              cursor: loading ? 'progress' : 'pointer',
              font: 'inherit',
              fontSize: 12,
              opacity: loading ? 0.6 : 1,
            }}
          >
            {loading ? 'Loading…' : 'Refresh'}
          </button>

          <button
            onClick={() => void handleDispatch()}
            disabled={dispatching}
            className="run-btn"
            style={{ padding: '4px 14px', fontSize: 12 }}
          >
            {dispatching ? 'Dispatching…' : 'Dispatch'}
          </button>
        </div>
      </header>

      {/* 404 / mock banner */}
      {isMock && (
        <div
          style={{
            background: '#fef3c7',
            border: '1px solid #fde68a',
            borderRadius: 8,
            padding: '10px 16px',
            marginBottom: 16,
            fontSize: 13,
            color: '#92400e',
          }}
        >
          <strong>Kanban board ships in v1.4 — design preview only. See ADR-0019.</strong>
          {' '}Showing demo data. Actions that mutate state are disabled against the live backend
          but work locally in this preview.
        </div>
      )}

      {/* Dispatch feedback */}
      {dispatchMsg && (
        <div
          style={{
            background: '#ecfdf5',
            border: '1px solid #a7f3d0',
            borderRadius: 6,
            padding: '8px 14px',
            marginBottom: 12,
            fontSize: 13,
            color: '#065f46',
          }}
        >
          {dispatchMsg}
        </div>
      )}

      {/* Error */}
      {error && <div className="error">{error}</div>}

      {/* Board */}
      <div
        style={{
          display: 'flex',
          gap: 10,
          overflowX: 'auto',
          alignItems: 'flex-start',
          minHeight: 'calc(100vh - 160px)',
        }}
      >
        {COLUMNS.map(({ key, label }) => (
          <KanbanColumn
            key={key}
            column={key}
            label={label}
            cards={byColumn(key)}
            onCardClick={(c) => void handleCardClick(c)}
            onDragStart={handleDragStart}
            onDrop={(col) => void handleDrop(col)}
          />
        ))}
      </div>

      {/* Detail drawer */}
      {selectedCard && (
        <TaskDrawer
          card={selectedCard}
          history={cardHistory}
          historyLoading={historyLoading}
          isMock={isMock}
          onClose={() => setSelectedCard(null)}
          onMoveToColumn={(col) => void handleMoveToColumn(col)}
          onBlock={(r) => void handleBlock(r)}
        />
      )}

      {/* New task modal */}
      {showNewTask && (
        <NewTaskModal
          boards={boards.length > 0 ? boards : MOCK_BOARDS}
          currentBoardId={activeBoardId}
          isMock={isMock}
          onClose={() => setShowNewTask(false)}
          onCreate={(req) => void handleCreateTask(req)}
        />
      )}
    </>
  );
}
