import { useEffect, useRef, useState, type ReactNode } from "react";
import { samples } from "../stores/sampleStore";
import { useApp } from "../stores/appStore";
import { useCal, type StatMode } from "../stores/calStore";
import { bilinear, fracIndex, outside } from "../lib/interp";
import { fmt } from "../lib/format";
import type { AxisBinding, TableData } from "../types/ms43";

interface Props {
  data: TableData;
  /** False when the loaded calibration does not match the connected ECU. */
  overlay: boolean;
}

interface Acc {
  hits: Float64Array;
  sum: Float64Array;
  lastT: number;
}

/** Calibration table heatmap with live operating-point overlay and per-cell log statistics. */
export function TableView({ data, overlay }: Props) {
  const { channelById, selected, select, status } = useApp();
  const { mode, statChannel, trailSec, set } = useCal();
  const [, tick] = useState(0);
  const rows = data.values.length;
  const cols = data.values[0]?.length ?? 0;
  const xb = cols > 1 ? data.x.binding : null;
  const yb = rows > 1 ? data.y.binding : null;
  const acc = useRef<Acc>({ hits: new Float64Array(rows * cols), sum: new Float64Array(rows * cols), lastT: -Infinity });

  const missing = [xb, yb].filter((b): b is AxisBinding => !!b && !selected.includes(b.channel)).map((b) => b.channel);
  const live = overlay && (xb || yb) && missing.length === 0 && status.state !== "Disconnected";

  // Reset accumulators when the table or the statistic changes, then fold in the session so far.
  useEffect(() => {
    acc.current = { hits: new Float64Array(rows * cols), sum: new Float64Array(rows * cols), lastT: -Infinity };
    accumulate();
    tick((n) => n + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.info.uid, statChannel, overlay]);

  // Incremental accumulation + throttled redraw on new samples.
  useEffect(() => {
    let pending = false;
    return samples.subscribe(() => {
      accumulate();
      if (pending) return;
      pending = true;
      setTimeout(() => {
        pending = false;
        tick((n) => n + 1);
      }, 150);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.info.uid, statChannel, overlay]);

  function cellOf(i: number): [number, number] | null {
    const xs = xb ? samples.series.get(xb.channel) : undefined;
    const ys = yb ? samples.series.get(yb.channel) : undefined;
    const xv = xs?.[i];
    const yv = ys?.[i];
    if ((xb && xv == null) || (yb && yv == null)) return null;
    const c = xb ? Math.round(fracIndex(xv!, data.x.values)) : 0;
    const r = yb ? Math.round(fracIndex(yv!, data.y.values)) : 0;
    return [r, c];
  }

  function accumulate() {
    if (!overlay || (!xb && !yb)) return;
    const a = acc.current;
    const st = samples.series.get(statChannel);
    const start = samples.indexFrom(a.lastT + 1e-9);
    for (let i = start; i < samples.t.length; i++) {
      const rc = cellOf(i);
      if (!rc) continue;
      const k = rc[0] * cols + rc[1];
      a.hits[k] += 1;
      const v = st?.[i];
      if (v != null) a.sum[k] += v;
    }
    if (samples.t.length) a.lastT = samples.t[samples.t.length - 1];
  }

  // Current operating point and trail.
  const xv = xb ? samples.last.get(xb.channel) : undefined;
  const yv = yb ? samples.last.get(yb.channel) : undefined;
  const point =
    live && (!xb || xv != null) && (!yb || yv != null)
      ? { fc: xb ? fracIndex(xv!, data.x.values) : 0, fr: yb ? fracIndex(yv!, data.y.values) : 0 }
      : null;
  const trail = (() => {
    if (!live || !samples.t.length) return "";
    const from = samples.indexFrom(samples.t[samples.t.length - 1] - trailSec);
    const xs = xb ? samples.series.get(xb.channel) : undefined;
    const ys = yb ? samples.series.get(yb.channel) : undefined;
    const pts: string[] = [];
    for (let i = from; i < samples.t.length; i++) {
      const x = xs?.[i];
      const y = ys?.[i];
      if ((xb && x == null) || (yb && y == null)) continue;
      const fc = xb ? fracIndex(x!, data.x.values) : 0;
      const fr = yb ? fracIndex(y!, data.y.values) : 0;
      pts.push(`${(fc + 0.5).toFixed(3)},${(fr + 0.5).toFixed(3)}`);
    }
    return pts.join(" ");
  })();
  const interp = point ? bilinear(data.values, point.fr, point.fc) : null;
  const near = point ? [Math.round(point.fr), Math.round(point.fc)] : null;

  // What each cell shows.
  const a = acc.current;
  const cellValue = (r: number, c: number): number | null => {
    const k = r * cols + c;
    switch (mode) {
      case "values":
        return data.values[r][c];
      case "hits":
        return a.hits[k] || null;
      case "mean":
        return a.hits[k] ? a.sum[k] / a.hits[k] : null;
      case "delta":
        return a.hits[k] ? a.sum[k] / a.hits[k] - data.values[r][c] : null;
    }
  };
  const all: number[] = [];
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) {
    const v = cellValue(r, c);
    if (v != null && Number.isFinite(v)) all.push(v);
  }
  const lo = Math.min(...all);
  const hi = Math.max(...all);
  const color = (v: number | null) => {
    if (v == null || !Number.isFinite(v) || !all.length) return "transparent";
    if (mode === "delta") {
      const m = Math.max(Math.abs(lo), Math.abs(hi)) || 1;
      const t = v / m; // -1..1
      return t >= 0 ? `hsla(0, 65%, 45%, ${0.15 + 0.6 * t})` : `hsla(215, 70%, 50%, ${0.15 - 0.6 * t})`;
    }
    const t = hi > lo ? (v - lo) / (hi - lo) : 0.5;
    return `hsl(${230 - 230 * t}, 55%, ${22 + 14 * t}%)`;
  };
  const statDef = channelById[statChannel];
  const decimals = mode === "hits" ? 0 : mode === "values" ? data.decimals : Math.max(1, data.decimals);
  const label = (b: AxisBinding | null, units: string) =>
    b ? `${units} ← ${channelById[b.channel]?.label ?? b.channel}${b.exact ? "" : " ≈"}` : units || "index";

  return (
    <div className="table-view">
      <div className="tv-head">
        <div>
          <div className="mono tv-title">{data.info.title}</div>
          <div className="muted small">{data.info.description}</div>
          <div className="muted small mono">
            {data.info.category} · {data.address} · {data.equation} · {data.info.units}
          </div>
        </div>
        <span className="spacer" />
        <div className="tv-modes">
          {(["values", "hits", "mean", "delta"] as StatMode[]).map((m) => (
            <button key={m} className={mode === m ? "active" : ""} onClick={() => set({ mode: m })} disabled={m !== "values" && !overlay}>
              {m === "values" ? "Table" : m === "hits" ? "Hits" : m === "mean" ? "Mean" : "Mean − table"}
            </button>
          ))}
          {(mode === "mean" || mode === "delta") && (
            <select value={statChannel} onChange={(e) => set({ statChannel: e.target.value })}>
              {selected.filter((id) => channelById[id]).map((id) => (
                <option key={id} value={id}>
                  {channelById[id].label} [{channelById[id].unit}]
                </option>
              ))}
            </select>
          )}
          <button
            className="ghost small"
            onClick={() => {
              acc.current = { hits: new Float64Array(rows * cols), sum: new Float64Array(rows * cols), lastT: samples.t.length ? samples.t[samples.t.length - 1] : -Infinity };
              tick((n) => n + 1);
            }}
          >
            reset stats
          </button>
        </div>
      </div>

      {!overlay && <div className="notice bad">Calibration does not match the connected ECU's software: live overlay and statistics are off.</div>}
      {overlay && !xb && !yb && <div className="notice warn">Neither axis of this table maps to a logged channel; table view only.</div>}
      {overlay && missing.length > 0 && (
        <div className="notice warn">
          Overlay needs {missing.join(", ")} in the logged channels.{" "}
          <button className="small" disabled={status.state === "Logging"} onClick={() => select([...selected, ...missing], null)}>
            add {missing.join(" + ")}
          </button>
        </div>
      )}

      <div className="tv-live mono small">
        {point ? (
          <>
            {xb && (
              <span>
                {channelById[xb.channel]?.label}: <b>{fmt(channelById[xb.channel], xv)}</b> {data.x.units}
                {outside(xv!, data.x.values) && <em className="warn"> (outside axis, clamped)</em>}
              </span>
            )}
            {yb && (
              <span>
                {channelById[yb.channel]?.label}: <b>{fmt(channelById[yb.channel], yv)}</b> {data.y.units}
                {outside(yv!, data.y.values) && <em className="warn"> (outside axis, clamped)</em>}
              </span>
            )}
            <span>
              table at point (interpolated): <b>{interp!.toFixed(data.decimals)}</b> {data.info.units}
            </span>
            {statDef && (mode === "mean" || mode === "delta") && (
              <span>
                {statDef.label}: <b>{fmt(statDef, samples.last.get(statChannel))}</b> {statDef.unit}
              </span>
            )}
          </>
        ) : (
          <span className="muted">{live ? "waiting for data…" : "not live"}</span>
        )}
        <span className="spacer" />
        <span className="muted">trail</span>
        {[0, 5, 15].map((s) => (
          <button key={s} className={`small ${trailSec === s ? "active" : ""}`} onClick={() => set({ trailSec: s })}>
            {s ? `${s}s` : "off"}
          </button>
        ))}
      </div>

      <div className="tv-grid" style={{ gridTemplateColumns: `auto repeat(${cols}, minmax(44px, 1fr))` }}>
        <div className="tv-corner muted small" style={{ gridRow: 1, gridColumn: 1 }}>{label(yb, data.y.units)} ↓ · {label(xb, data.x.units)} →</div>
        {data.x.values.map((x, c) => (
          <div key={c} className="tv-xh mono" style={{ gridRow: 1, gridColumn: c + 2 }}>
            {round(x)}
          </div>
        ))}
        {data.values.map((row, r) => (
          <Row key={r}>
            <div className="tv-yh mono" style={{ gridRow: r + 2, gridColumn: 1 }}>
              {round(data.y.values[r])}
            </div>
            {row.map((_, c) => {
              const v = cellValue(r, c);
              const isNear = near && near[0] === r && near[1] === c;
              return (
                <div
                  key={c}
                  className={`tv-cell mono ${isNear ? "near" : ""}`}
                  style={{ background: color(v), gridRow: r + 2, gridColumn: c + 2 }}
                  title={`${data.y.units} ${round(data.y.values[r])}, ${data.x.units} ${round(data.x.values[c])}\ntable ${data.values[r][c].toFixed(data.decimals)} ${data.info.units} (raw ${data.raw[r][c]})\nhits ${a.hits[r * cols + c]}${a.hits[r * cols + c] && statDef ? `\nmean ${statDef.label} ${(a.sum[r * cols + c] / a.hits[r * cols + c]).toFixed(2)}` : ""}`}
                >
                  {v == null ? "" : mode === "delta" && v > 0 ? `+${v.toFixed(decimals)}` : v.toFixed(decimals)}
                </div>
              );
            })}
          </Row>
        ))}
        {live && (
          <div className="tv-overlay" style={{ gridColumn: `2 / span ${cols}`, gridRow: `2 / span ${rows}` }}>
            <svg viewBox={`0 0 ${cols} ${rows}`} preserveAspectRatio="none">
              {trail && (
                <polyline points={trail} fill="none" stroke="#ffffffaa" strokeWidth={1.5} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
              )}
              {point && (
                <>
                  <line x1={point.fc + 0.5} x2={point.fc + 0.5} y1={0} y2={rows} stroke="#ffffff55" vectorEffect="non-scaling-stroke" />
                  <line y1={point.fr + 0.5} y2={point.fr + 0.5} x1={0} x2={cols} stroke="#ffffff55" vectorEffect="non-scaling-stroke" />
                </>
              )}
            </svg>
            {point && (
              <div
                className="tv-dot"
                style={{ left: `${((point.fc + 0.5) / cols) * 100}%`, top: `${((point.fr + 0.5) / rows) * 100}%` }}
              />
            )}
          </div>
        )}
      </div>
      <div className="muted small tv-foot">
        Bindings: {xb ? `${xb.var} → ${xb.channel}${xb.exact ? "" : ` (≈ ${xb.note})`}` : "x unbound"} ·{" "}
        {yb ? `${yb.var} → ${yb.channel}${yb.exact ? "" : ` (≈ ${yb.note})`}` : "y unbound"}. Shows where you are on this
        table; MS43 may be using a different table for the same output (warm/cold, octane, VANOS fault).
      </div>
    </div>
  );
}

function Row({ children }: { children: ReactNode }) {
  return <>{children}</>;
}

const round = (v: number) => (Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 10 ? v.toFixed(1) : v.toFixed(2));
