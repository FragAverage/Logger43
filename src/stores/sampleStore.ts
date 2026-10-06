// Non-reactive sample buffer. Rows arrive in batches (~10/s); charts subscribe and redraw on
// each batch instead of React re-rendering per sample.
import type { SampleBatch } from "../types/ms43";

const MAX_POINTS = 30_000; // ~30 min at 15 Hz

type Listener = () => void;

class SampleStore {
  t: number[] = [];
  series = new Map<string, (number | null)[]>();
  last = new Map<string, number>();
  min = new Map<string, number>();
  max = new Map<string, number>();
  version = 0;
  private listeners = new Set<Listener>();

  append(b: SampleBatch) {
    const n = b.t.length;
    if (n === 0) return;
    const before = this.t.length;
    for (const v of b.t) this.t.push(v);
    // keep every known series aligned with t
    b.columns.forEach((id, ci) => {
      let s = this.series.get(id);
      if (!s) {
        s = new Array(before).fill(null);
        this.series.set(id, s);
      }
      const col = b.values[ci];
      for (let i = 0; i < n; i++) {
        const v = col[i];
        s.push(v);
        if (v != null) {
          this.last.set(id, v);
          if (!(this.min.get(id)! <= v)) this.min.set(id, v);
          if (!(this.max.get(id)! >= v)) this.max.set(id, v);
        }
      }
    });
    for (const [id, s] of this.series) {
      if (!b.columns.includes(id)) for (let i = 0; i < n; i++) s.push(null);
    }
    if (this.t.length > MAX_POINTS * 1.2) {
      const cut = this.t.length - MAX_POINTS;
      this.t.splice(0, cut);
      for (const s of this.series.values()) s.splice(0, cut);
    }
    this.bump();
  }

  /** Index of the first point with t >= from. */
  indexFrom(from: number): number {
    let lo = 0;
    let hi = this.t.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.t[mid] < from) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  resetMinMax() {
    this.min.clear();
    this.max.clear();
    this.bump();
  }

  clear() {
    this.t = [];
    this.series.clear();
    this.last.clear();
    this.resetMinMax();
  }

  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  private bump() {
    this.version++;
    this.listeners.forEach((l) => l());
  }
}

export const samples = new SampleStore();
