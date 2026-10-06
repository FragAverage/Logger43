import { useEffect, useMemo, useRef, useState } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { samples } from "../stores/sampleStore";
import { useApp, type ChartConfig } from "../stores/appStore";
import { decimals, PALETTE } from "../lib/format";

interface Props {
  config: ChartConfig;
}

/** Streaming time-series chart. Live: follows the last `windowSec`. Drag to zoom (pauses). Double-click: full range. */
export function LiveChart({ config }: Props) {
  const { selected, channelById, paused, windowSec, set, setChartChannels } = useApp();
  const host = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const [editing, setEditing] = useState(false);

  const visible = useMemo(
    () => config.channels.filter((id) => selected.includes(id) && channelById[id]),
    [config.channels, selected, channelById],
  );

  // (Re)build the plot when the series list changes.
  useEffect(() => {
    const el = host.current;
    if (!el) return;
    plot.current?.destroy();
    plot.current = null;
    if (visible.length === 0) return;

    const units: string[] = [];
    for (const id of visible) {
      const u = channelById[id].unit;
      if (!units.includes(u)) units.push(u);
    }
    const scaleOf = (id: string) => (units.indexOf(channelById[id].unit) === 0 ? "y" : "y2");
    const axisStyle = { stroke: "#7d8590", grid: { stroke: "#23272d", width: 1 }, ticks: { stroke: "#23272d" }, font: "11px ui-monospace, Menlo, Consolas, monospace" };

    const opts: uPlot.Options = {
      width: el.clientWidth,
      height: Math.max(120, el.clientHeight),
      pxAlign: 0,
      scales: { x: { time: false }, y: { auto: true }, y2: { auto: true } },
      axes: [
        { ...axisStyle, values: (_u, ticks) => ticks.map((t) => `${t.toFixed(0)}s`) },
        { ...axisStyle, scale: "y", label: units[0] || undefined, labelSize: 14, labelFont: "11px sans-serif", size: 50 },
        ...(units.length > 1
          ? [{ ...axisStyle, scale: "y2", side: 1 as const, grid: { show: false }, label: units.slice(1).join(" / "), labelSize: 14, labelFont: "11px sans-serif", size: 50 }]
          : []),
      ],
      series: [
        { label: "t", value: (_u, v) => (v == null ? "—" : `${v.toFixed(2)} s`) },
        ...visible.map((id, i) => {
          const c = channelById[id];
          const dash = id.endsWith("_target") ? [6, 4] : undefined;
          return {
            label: `${c.label} [${c.unit}]`,
            scale: scaleOf(id),
            stroke: PALETTE[i % PALETTE.length],
            width: 1.5,
            dash,
            spanGaps: false,
            points: { show: false },
            value: (_u: uPlot, v: number | null) => (v == null ? "—" : v.toFixed(decimals(c))),
          };
        }),
      ],
      cursor: { drag: { x: true, y: false, setScale: true }, points: { size: 5 } },
      legend: { live: true },
      hooks: {
        setSelect: [
          (u) => {
            if (u.select.width > 2) useApp.getState().set({ paused: true });
          },
        ],
      },
    };
    plot.current = new uPlot(opts, [[], ...visible.map(() => [])] as uPlot.AlignedData, el);
    const fit = () =>
      plot.current?.setSize({ width: el.clientWidth, height: Math.max(80, el.clientHeight - legendHeight(el) - 4) });
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    draw(true);
    return () => {
      ro.disconnect();
      plot.current?.destroy();
      plot.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible.join(","), channelById]);

  function draw(full: boolean) {
    const u = plot.current;
    if (!u) return;
    const n = samples.t.length;
    const tMax = n ? samples.t[n - 1] : 0;
    const from = full ? 0 : samples.indexFrom(tMax - windowSec);
    const x = samples.t.slice(from);
    const ys = visible.map((id) => (samples.series.get(id) ?? []).slice(from));
    u.setData([x, ...ys] as uPlot.AlignedData, false);
    u.setScale("x", { min: Math.max(x[0] ?? 0, tMax - windowSec), max: Math.max(tMax, windowSec * 0.1) });
  }

  // Live updates on every batch; paused shows the whole buffer for zooming.
  useEffect(() => {
    if (paused) {
      draw(true);
      return;
    }
    draw(false);
    return samples.subscribe(() => draw(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paused, windowSec, visible.join(",")]);

  const candidates = selected.filter((id) => channelById[id]);

  return (
    <div className="chart">
      <div className="chart-head">
        <span className="chart-title">{config.title}</span>
        <span className="spacer" />
        <button className="ghost small" onClick={() => setEditing(!editing)} title="Choose channels">
          {editing ? "done" : "channels"}
        </button>
        <button className="ghost small" onClick={() => set({ paused: !paused })}>
          {paused ? "resume" : "pause"}
        </button>
      </div>
      {editing && (
        <div className="chart-edit">
          {candidates.map((id) => (
            <label key={id}>
              <input
                type="checkbox"
                checked={config.channels.includes(id)}
                onChange={(e) =>
                  setChartChannels(
                    config.id,
                    e.target.checked ? [...config.channels, id] : config.channels.filter((c) => c !== id),
                  )
                }
              />
              {channelById[id].label}
            </label>
          ))}
        </div>
      )}
      <div className="chart-body" ref={host} onDoubleClick={() => draw(true)}>
        {visible.length === 0 && <div className="empty">No selected channel in this chart — use “channels”.</div>}
      </div>
    </div>
  );
}

function legendHeight(el: HTMLElement): number {
  const lg = el.querySelector(".u-legend") as HTMLElement | null;
  return lg ? lg.offsetHeight : 0;
}
