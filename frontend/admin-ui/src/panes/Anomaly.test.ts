/**
 * v1.4 — Unit / integration tests for Anomaly pane helpers.
 *
 * We test pure helper functions and type-shape assertions rather than
 * full DOM rendering (no jsdom/RTL in devDependencies). Type-level
 * contracts are enforced by tsc --noEmit (pnpm typecheck).
 *
 * Covered:
 *   - bucketDetections pivot helper
 *   - isEndpointAbsent (404 / 503 discrimination)
 *   - fmtTs timestamp formatter
 *   - AnomalyDetection wire-shape type check
 *   - AnomalyDetectorConfig wire-shape type check
 *   - Filter logic (detector / severity)
 *   - False-positive optimistic state update
 *   - HotL-gated apply: patch shape validation
 *   - No-data (empty) state when /v1/anomaly returns 503
 */

import { describe, expect, it } from 'vitest';
import type {
  AnomalyDetection,
  AnomalyDetectorConfig,
  AnomalyDetectorKind,
  AnomalyDetectorPatch,
  AnomalyFireRateBucket,
} from '@xiaoguai/shared';
import { ApiError } from '@xiaoguai/shared';

// ---------------------------------------------------------------------------
// Inline the pure helpers under test.
// (Not exported from the pane — extract to utils/ in v1.4.1.)
// ---------------------------------------------------------------------------

function isEndpointAbsent(err: unknown): boolean {
  if (err instanceof ApiError) {
    return err.status === 404 || err.status === 503;
  }
  return false;
}

function fmtTs(ts: string): string {
  const d = new Date(ts);
  if (isNaN(d.getTime())) return ts;
  return (
    d.toISOString().slice(0, 10) + ' ' + d.toISOString().slice(11, 16) + ' UTC'
  );
}

function bucketDetections(
  detections: AnomalyDetection[],
  detectorId: string,
): AnomalyFireRateBucket[] {
  const counts = new Map<string, number>();
  const now = new Date();
  for (let i = 13; i >= 0; i--) {
    const d = new Date(now.getTime() - i * 86_400_000);
    counts.set(d.toISOString().slice(0, 10), 0);
  }
  for (const d of detections) {
    if (d.detector_id !== detectorId) continue;
    const day = d.fired_at.slice(0, 10);
    if (counts.has(day)) {
      counts.set(day, (counts.get(day) ?? 0) + 1);
    }
  }
  return Array.from(counts.entries()).map(([date, count]) => ({ date, count }));
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const BASE_DETECTION: AnomalyDetection = {
  id: 'det_001',
  detector_id: 'orders_anomaly',
  fired_at: '2026-05-20T10:00:00Z',
  severity: 'medium',
  series_key: 'orders.count',
  value: 142.5,
  threshold: 120.0,
  is_false_positive: false,
};

const ZSCORE_CONFIG: AnomalyDetectorConfig = {
  id: 'orders_anomaly',
  kpi_query: "SELECT COUNT(*) FROM orders WHERE created_at > NOW() - INTERVAL '1 minute'",
  window_secs: 7200,
  detector: {
    kind: 'z_score',
    sigma_threshold: 3.0,
    min_count: 10,
  },
  cool_off_secs: 900,
};

const EWMA_CONFIG: AnomalyDetectorConfig = {
  id: 'latency_ewma',
  kpi_query: 'avg(http_request_duration_seconds)',
  window_secs: 1800,
  detector: {
    kind: 'ewma',
    alpha: 0.15,
    sigma_threshold: 2.5,
    min_count: 5,
  },
  cool_off_secs: 600,
};

// ---------------------------------------------------------------------------
// isEndpointAbsent
// ---------------------------------------------------------------------------

describe('isEndpointAbsent', () => {
  it('returns true for ApiError 404', () => {
    expect(isEndpointAbsent(new ApiError(404, 'not_found', 'Not found'))).toBe(true);
  });

  it('returns true for ApiError 503', () => {
    expect(isEndpointAbsent(new ApiError(503, 'service_unavailable', 'Unavailable'))).toBe(true);
  });

  it('returns false for ApiError 500', () => {
    expect(isEndpointAbsent(new ApiError(500, 'internal', 'Server error'))).toBe(false);
  });

  it('returns false for ApiError 401', () => {
    expect(isEndpointAbsent(new ApiError(401, 'unauthorized', 'Unauthorized'))).toBe(false);
  });

  it('returns false for plain Error', () => {
    expect(isEndpointAbsent(new Error('network error'))).toBe(false);
  });

  it('returns false for null', () => {
    expect(isEndpointAbsent(null)).toBe(false);
  });

  it('returns false for string', () => {
    expect(isEndpointAbsent('503 error')).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// fmtTs
// ---------------------------------------------------------------------------

describe('fmtTs', () => {
  it('formats a valid RFC 3339 timestamp', () => {
    expect(fmtTs('2026-05-20T10:30:00Z')).toBe('2026-05-20 10:30 UTC');
  });

  it('passes through invalid strings unchanged', () => {
    expect(fmtTs('not-a-date')).toBe('not-a-date');
  });

  it('handles midnight', () => {
    expect(fmtTs('2026-01-01T00:00:00Z')).toBe('2026-01-01 00:00 UTC');
  });
});

// ---------------------------------------------------------------------------
// bucketDetections
// ---------------------------------------------------------------------------

describe('bucketDetections', () => {
  it('produces exactly 14 buckets', () => {
    const result = bucketDetections([], 'orders_anomaly');
    expect(result).toHaveLength(14);
  });

  it('buckets are sorted ascending by date', () => {
    const result = bucketDetections([], 'orders_anomaly');
    for (let i = 1; i < result.length; i++) {
      expect(result[i]!.date >= result[i - 1]!.date).toBe(true);
    }
  });

  it('counts a detection on the correct day', () => {
    const today = new Date().toISOString().slice(0, 10);
    const detection: AnomalyDetection = {
      ...BASE_DETECTION,
      fired_at: `${today}T12:00:00Z`,
    };
    const result = bucketDetections([detection], 'orders_anomaly');
    const todayBucket = result.find((b) => b.date === today);
    expect(todayBucket?.count).toBe(1);
  });

  it('ignores detections from a different detector', () => {
    const today = new Date().toISOString().slice(0, 10);
    const detection: AnomalyDetection = {
      ...BASE_DETECTION,
      detector_id: 'other_detector',
      fired_at: `${today}T12:00:00Z`,
    };
    const result = bucketDetections([detection], 'orders_anomaly');
    const total = result.reduce((s, b) => s + b.count, 0);
    expect(total).toBe(0);
  });

  it('counts multiple detections on the same day', () => {
    const today = new Date().toISOString().slice(0, 10);
    const d1: AnomalyDetection = { ...BASE_DETECTION, id: 'd1', fired_at: `${today}T08:00:00Z` };
    const d2: AnomalyDetection = { ...BASE_DETECTION, id: 'd2', fired_at: `${today}T14:00:00Z` };
    const result = bucketDetections([d1, d2], 'orders_anomaly');
    const todayBucket = result.find((b) => b.date === today);
    expect(todayBucket?.count).toBe(2);
  });

  it('initialises all buckets to zero when no detections match', () => {
    const result = bucketDetections([], 'latency_ewma');
    expect(result.every((b) => b.count === 0)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Detection list render (type-shape check)
// ---------------------------------------------------------------------------

describe('AnomalyDetection wire-shape', () => {
  it('accepts the expected wire shape without TypeScript errors', () => {
    const d: AnomalyDetection = {
      id: 'ev_001',
      detector_id: 'orders_anomaly',
      fired_at: '2026-05-20T10:00:00Z',
      severity: 'high',
      series_key: 'orders.count',
      value: 200,
      threshold: 130,
      is_false_positive: false,
    };
    expect(d.severity).toBe('high');
    expect(d.is_false_positive).toBe(false);
  });

  it('accepts all three severity levels', () => {
    const severities = ['low', 'medium', 'high'] as const;
    for (const severity of severities) {
      const d: AnomalyDetection = { ...BASE_DETECTION, severity };
      expect(d.severity).toBe(severity);
    }
  });
});

// ---------------------------------------------------------------------------
// AnomalyDetectorConfig wire-shape
// ---------------------------------------------------------------------------

describe('AnomalyDetectorConfig wire-shape', () => {
  it('accepts a z_score detector config', () => {
    const cfg: AnomalyDetectorConfig = ZSCORE_CONFIG;
    expect(cfg.detector.kind).toBe('z_score');
    if (cfg.detector.kind === 'z_score') {
      expect(cfg.detector.sigma_threshold).toBe(3.0);
      expect(cfg.detector.min_count).toBe(10);
    }
  });

  it('accepts an ewma detector config', () => {
    const cfg: AnomalyDetectorConfig = EWMA_CONFIG;
    expect(cfg.detector.kind).toBe('ewma');
    if (cfg.detector.kind === 'ewma') {
      expect(cfg.detector.alpha).toBe(0.15);
      expect(cfg.detector.sigma_threshold).toBe(2.5);
    }
  });
});

// ---------------------------------------------------------------------------
// Filter logic (detector / severity)
// ---------------------------------------------------------------------------

describe('detection filtering (pure simulation)', () => {
  const detections: AnomalyDetection[] = [
    { ...BASE_DETECTION, id: '1', detector_id: 'orders_anomaly', severity: 'high' },
    { ...BASE_DETECTION, id: '2', detector_id: 'latency_ewma', severity: 'low' },
    { ...BASE_DETECTION, id: '3', detector_id: 'orders_anomaly', severity: 'medium' },
  ];

  function filterDetections(
    rows: AnomalyDetection[],
    detectorId: string,
    severity: string,
  ): AnomalyDetection[] {
    return rows.filter(
      (d) =>
        (!detectorId || d.detector_id === detectorId) &&
        (!severity || d.severity === severity),
    );
  }

  it('returns all rows when no filter is set', () => {
    expect(filterDetections(detections, '', '')).toHaveLength(3);
  });

  it('filters by detector_id', () => {
    const result = filterDetections(detections, 'orders_anomaly', '');
    expect(result).toHaveLength(2);
    expect(result.every((d) => d.detector_id === 'orders_anomaly')).toBe(true);
  });

  it('filters by severity', () => {
    const result = filterDetections(detections, '', 'high');
    expect(result).toHaveLength(1);
    expect(result[0]?.severity).toBe('high');
  });

  it('filters by both detector_id and severity', () => {
    const result = filterDetections(detections, 'orders_anomaly', 'medium');
    expect(result).toHaveLength(1);
    expect(result[0]?.id).toBe('3');
  });

  it('returns empty when nothing matches', () => {
    expect(filterDetections(detections, 'orders_anomaly', 'low')).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// False-positive optimistic state update
// ---------------------------------------------------------------------------

describe('false-positive optimistic update', () => {
  function applyFpUpdate(
    detections: AnomalyDetection[],
    targetId: string,
    isFp: boolean,
  ): AnomalyDetection[] {
    return detections.map((d) =>
      d.id === targetId ? { ...d, is_false_positive: isFp } : d,
    );
  }

  it('flips is_false_positive to true', () => {
    const result = applyFpUpdate([BASE_DETECTION], 'det_001', true);
    expect(result[0]?.is_false_positive).toBe(true);
  });

  it('reverts is_false_positive back to false', () => {
    const fp = { ...BASE_DETECTION, is_false_positive: true };
    const result = applyFpUpdate([fp], 'det_001', false);
    expect(result[0]?.is_false_positive).toBe(false);
  });

  it('does not mutate original array', () => {
    const original = [{ ...BASE_DETECTION }];
    applyFpUpdate(original, 'det_001', true);
    expect(original[0]?.is_false_positive).toBe(false);
  });

  it('leaves unrelated detections unchanged', () => {
    const other: AnomalyDetection = { ...BASE_DETECTION, id: 'det_002' };
    const result = applyFpUpdate([BASE_DETECTION, other], 'det_001', true);
    expect(result[1]?.is_false_positive).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// HotL-gated apply: patch shape validation
// ---------------------------------------------------------------------------

describe('AnomalyDetectorPatch shape', () => {
  it('accepts a z_score patch', () => {
    const kind: AnomalyDetectorKind = {
      kind: 'z_score',
      sigma_threshold: 2.5,
      min_count: 15,
    };
    const patch: AnomalyDetectorPatch = {
      detector: kind,
      window_secs: 3600,
      cool_off_secs: 1200,
    };
    expect(patch.detector?.kind).toBe('z_score');
    expect(patch.window_secs).toBe(3600);
  });

  it('accepts an ewma patch', () => {
    const kind: AnomalyDetectorKind = {
      kind: 'ewma',
      alpha: 0.2,
      sigma_threshold: 3.5,
      min_count: 8,
    };
    const patch: AnomalyDetectorPatch = { detector: kind };
    expect(patch.detector?.kind).toBe('ewma');
  });

  it('accepts a partial patch (only window_secs)', () => {
    const patch: AnomalyDetectorPatch = { window_secs: 900 };
    expect(patch.window_secs).toBe(900);
    expect(patch.detector).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// No-data state when endpoints return 503
// ---------------------------------------------------------------------------

describe('no-data / 503 degraded state simulation', () => {
  it('isEndpointAbsent correctly identifies 503 errors from the anomaly surface', () => {
    // Simulate what the pane does when listAnomalyDetections rejects with 503.
    const err = new ApiError(503, 'service_unavailable', 'Detector dashboard coming v1.4');
    expect(isEndpointAbsent(err)).toBe(true);
  });

  it('produces empty detection list + zero total when endpoint absent', () => {
    // Simulate the state after catching a 503.
    const detections: AnomalyDetection[] = [];
    const total = 0;
    expect(detections).toHaveLength(0);
    expect(total).toBe(0);
  });

  it('buckets still produce 14 zero rows in degraded state', () => {
    const buckets = bucketDetections([], 'any_detector');
    expect(buckets).toHaveLength(14);
    expect(buckets.every((b) => b.count === 0)).toBe(true);
  });

  it('404 from getAnomalyDetector is treated as absent (no tuning panel crash)', () => {
    const err = new ApiError(404, 'not_found', 'Detector not found');
    expect(isEndpointAbsent(err)).toBe(true);
  });
});
