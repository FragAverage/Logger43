import { LiveChart } from "../charts/LiveChart";
import { Readouts } from "../components/Readouts";
import { samples } from "../stores/sampleStore";
import { useApp } from "../stores/appStore";

export function LivePage() {
  const { charts, paused, windowSec, set } = useApp();
  return (
    <div className="live">
      <Readouts />
      <div className="chart-toolbar">
        <button className={paused ? "active" : ""} onClick={() => set({ paused: !paused })}>
          {paused ? "▶ Resume" : "❚❚ Pause"}
        </button>
        <button
          onClick={() => {
            samples.clear();
            set({ paused: false });
          }}
        >
          Reset
        </button>
        <span className="muted small">window</span>
        {[10, 30, 60, 120].map((w) => (
          <button key={w} className={windowSec === w ? "active" : ""} onClick={() => set({ windowSec: w })}>
            {w}s
          </button>
        ))}
        <span className="muted small">drag on a chart to zoom (pauses) · double-click for full range · hover for values</span>
      </div>
      <div className="chart-grid">
        {charts.map((c) => (
          <LiveChart key={c.id} config={c} />
        ))}
      </div>
    </div>
  );
}
