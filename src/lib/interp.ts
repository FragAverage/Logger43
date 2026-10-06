// Axis lookup and interpolation helpers for table overlays.

/** Fractional index of v on a monotonic breakpoint axis, clamped to [0, n-1]. */
export function fracIndex(v: number, bps: number[]): number {
  const n = bps.length;
  if (n < 2 || !Number.isFinite(v)) return 0;
  const inc = bps[n - 1] >= bps[0];
  const a = (i: number) => (inc ? bps[i] : -bps[i]);
  const x = inc ? v : -v;
  if (x <= a(0)) return 0;
  if (x >= a(n - 1)) return n - 1;
  let i = 0;
  while (i < n - 2 && x >= a(i + 1)) i++;
  const span = a(i + 1) - a(i);
  return span === 0 ? i : i + (x - a(i)) / span;
}

/** Bilinear interpolation at fractional (row, col). */
export function bilinear(values: number[][], fr: number, fc: number): number {
  const r0 = Math.floor(fr);
  const c0 = Math.floor(fc);
  const r1 = Math.min(r0 + 1, values.length - 1);
  const c1 = Math.min(c0 + 1, values[0].length - 1);
  const tr = fr - r0;
  const tc = fc - c0;
  const top = values[r0][c0] * (1 - tc) + values[r0][c1] * tc;
  const bot = values[r1][c0] * (1 - tc) + values[r1][c1] * tc;
  return top * (1 - tr) + bot * tr;
}

/** True when v lies outside the axis range (the ECU clamps to the edge cell). */
export const outside = (v: number, bps: number[]) =>
  v < Math.min(bps[0], bps[bps.length - 1]) || v > Math.max(bps[0], bps[bps.length - 1]);
