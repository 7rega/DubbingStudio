/**
 * Waveform and Timeline utility functions.
 * Optimized for high performance and viewport virtualization.
 */

export function clampTime(time: number, duration: number): number {
  if (!Number.isFinite(time)) return 0;
  const dur = Math.max(0, Number.isFinite(duration) ? duration : 0);
  return Math.max(0, Math.min(time, dur));
}

export function decimatePeaks(peaks: number[], maxBars: number): number[] {
  if (!peaks || peaks.length === 0) return [];
  if (peaks.length <= maxBars) return peaks;
  const out = new Array<number>(maxBars);
  const n = peaks.length;
  for (let i = 0; i < maxBars; i++) {
    const a = Math.floor((i * n) / maxBars);
    const b = Math.max(a + 1, Math.floor(((i + 1) * n) / maxBars));
    let max = 0;
    for (let j = a; j < b; j++) {
      const p = peaks[j];
      if (p != null && p > max) max = p;
    }
    out[i] = max;
  }
  return out;
}

export function mergeIntervals(
  intervals: Array<{ start: number; end: number }>
): Array<{ start: number; end: number }> {
  if (!intervals || intervals.length === 0) return [];
  const valid = intervals
    .filter((s) => Number.isFinite(s.start) && Number.isFinite(s.end) && s.end >= s.start)
    .sort((a, b) => a.start - b.start);
  if (valid.length === 0) return [];

  const merged: Array<{ start: number; end: number }> = [
    { start: valid[0].start, end: valid[0].end },
  ];
  for (let i = 1; i < valid.length; i++) {
    const prev = merged[merged.length - 1];
    const curr = valid[i];
    if (curr.start <= prev.end) {
      prev.end = Math.max(prev.end, curr.end);
    } else {
      merged.push({ start: curr.start, end: curr.end });
    }
  }
  return merged;
}

export function computeFallbackPeaks(
  basePeaks: number[],
  segments: Array<{ start: number; end: number }>,
  totalDuration: number,
  kind: "dub" | "bgm"
): number[] {
  const n = basePeaks.length;
  if (n === 0) return [];
  const safeTotal = Math.max(0.001, totalDuration || 1);
  const step = safeTotal / n;
  const merged = mergeIntervals(segments);

  const out = new Array<number>(n);
  let intervalIdx = 0;

  for (let i = 0; i < n; i++) {
    const t = i * step;
    while (intervalIdx < merged.length && merged[intervalIdx].end < t) {
      intervalIdx++;
    }
    const insideSeg =
      intervalIdx < merged.length &&
      t >= merged[intervalIdx].start &&
      t <= merged[intervalIdx].end;

    const val = basePeaks[i] || 0;
    if (kind === "dub") {
      out[i] = insideSeg ? Math.min(1.0, val * 1.15 + 0.06) : 0.015;
    } else {
      // bgm
      out[i] = insideSeg ? Math.max(0.04, val * 0.4) : Math.min(1.0, val * 0.95 + 0.08);
    }
  }
  return out;
}

export type VisibleTileRange = {
  startTile: number;
  endTile: number;
  totalTiles: number;
};

export function computeVisibleTileRange(
  scrollLeft: number,
  clientWidth: number,
  totalPx: number,
  tileWidth: number,
  bufferTiles = 1
): VisibleTileRange {
  if (!Number.isFinite(totalPx) || totalPx <= 0 || !Number.isFinite(tileWidth) || tileWidth <= 0) {
    return { startTile: 0, endTile: 0, totalTiles: 0 };
  }
  const totalTiles = Math.ceil(totalPx / tileWidth);
  const safeScroll = Math.max(0, Number.isFinite(scrollLeft) ? scrollLeft : 0);
  const safeWidth = Math.max(0, Number.isFinite(clientWidth) ? clientWidth : 0);

  const minVisibleX = safeScroll - bufferTiles * tileWidth;
  const maxVisibleX = safeScroll + safeWidth + bufferTiles * tileWidth;

  const rawStart = Math.floor(minVisibleX / tileWidth);
  const rawEnd = Math.floor(maxVisibleX / tileWidth);

  const startTile = Math.max(0, Math.min(totalTiles - 1, rawStart));
  const endTile = Math.max(startTile, Math.min(totalTiles - 1, rawEnd));

  return { startTile, endTile, totalTiles };
}

