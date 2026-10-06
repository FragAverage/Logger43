import { useEffect, useState } from "react";
import { samples } from "../stores/sampleStore";
import { useApp } from "../stores/appStore";
import { fmt } from "../lib/format";

/** Compact digital gauges for every selected channel; RPM gets a bar. Throttled to ~5 Hz. */
export function Readouts() {
  const { selected, channelById } = useApp();
  const [, tick] = useState(0);

  useEffect(() => {
    let pending = false;
    return samples.subscribe(() => {
      if (pending) return;
      pending = true;
      setTimeout(() => {
        pending = false;
        tick((n) => n + 1);
      }, 200);
    });
  }, []);

  const rpm = samples.last.get("rpm");
  return (
    <div className="readouts">
      {selected
        .filter((id) => channelById[id])
        .map((id) => {
          const c = channelById[id];
          const v = samples.last.get(id);
          return (
            <div key={id} className={`readout ${id === "rpm" ? "wide" : ""}`} title={`${c.label}\nmin ${fmt(c, samples.min.get(id))} / max ${fmt(c, samples.max.get(id))}`}>
              <div className="r-label">
                {c.label}
                <i className={`dot ${c.verification}`} />
              </div>
              <div className="r-value mono">
                {fmt(c, v)}
                <span className="r-unit">{c.unit}</span>
              </div>
              {id === "rpm" ? (
                <div className="rpm-bar">
                  <div style={{ width: `${Math.min(100, ((rpm ?? 0) / 7000) * 100)}%` }} className={(rpm ?? 0) > 6200 ? "hot" : ""} />
                </div>
              ) : (
                <div className="r-minmax mono">
                  {fmt(c, samples.min.get(id))} … {fmt(c, samples.max.get(id))}
                </div>
              )}
            </div>
          );
        })}
      <button className="ghost small reset-mm" onClick={() => samples.resetMinMax()} title="Reset min/max">
        reset min/max
      </button>
    </div>
  );
}
