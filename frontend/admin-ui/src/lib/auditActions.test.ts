/**
 * feat(single-owner-ux) — tests for the audit-action categorisation helpers.
 *
 * Pure functions, no rendering: assert a representative prefix→category
 * mapping, the multi-prefix buckets (git→code, team→orchestration), the
 * `other` fallback for unknown/empty actions, and the i18n key shaping.
 */

import { describe, expect, it } from 'vitest';
import {
  auditCategory,
  actionLabelKey,
  AUDIT_FILTER_CATEGORIES,
} from './auditActions';

describe('auditCategory', () => {
  it('maps known prefixes to their category', () => {
    expect(auditCategory('session.create')).toBe('session');
    expect(auditCategory('tool.invoke')).toBe('tool');
    expect(auditCategory('auth.login')).toBe('auth');
    expect(auditCategory('memory.recall')).toBe('memory');
    expect(auditCategory('hotl.escalate')).toBe('approval');
    expect(auditCategory('cost.charge')).toBe('cost');
    expect(auditCategory('policy.deny')).toBe('policy');
    expect(auditCategory('audit.verify')).toBe('audit');
    expect(auditCategory('data.export')).toBe('data');
    expect(auditCategory('consent.grant')).toBe('consent');
    expect(auditCategory('incident.open')).toBe('incident');
    expect(auditCategory('skill.install')).toBe('skill');
    expect(auditCategory('agent.run')).toBe('agent');
  });

  it('folds related prefixes into one category', () => {
    expect(auditCategory('code.edit')).toBe('code');
    expect(auditCategory('git.commit')).toBe('code');
    expect(auditCategory('orchestration.start')).toBe('orchestration');
    expect(auditCategory('team.run')).toBe('orchestration');
    expect(auditCategory('loop.iterate')).toBe('orchestration');
  });

  it('falls back to "other" for unknown or empty prefixes', () => {
    expect(auditCategory('quux.frobnicate')).toBe('other');
    expect(auditCategory('weird')).toBe('other');
    expect(auditCategory('')).toBe('other');
  });
});

describe('actionLabelKey', () => {
  it('replaces every dot with an underscore', () => {
    expect(actionLabelKey('session.create')).toBe('session_create');
    expect(actionLabelKey('memory.recall')).toBe('memory_recall');
    expect(actionLabelKey('a.b.c')).toBe('a_b_c');
    expect(actionLabelKey('nodots')).toBe('nodots');
  });
});

describe('AUDIT_FILTER_CATEGORIES', () => {
  it('starts with "all" and only lists concrete buckets', () => {
    expect(AUDIT_FILTER_CATEGORIES[0]).toBe('all');
    expect(AUDIT_FILTER_CATEGORIES).toContain('session');
    expect(AUDIT_FILTER_CATEGORIES).toContain('other');
    // No duplicates.
    expect(new Set(AUDIT_FILTER_CATEGORIES).size).toBe(
      AUDIT_FILTER_CATEGORIES.length,
    );
  });
});
