import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useApp } from "../stores/appStore";
import { fmtDuration } from "../lib/format";

export function LogPanel() {
  const { status, stats, activePreset, lastError, set } = useApp();
  const [runName, setRunName] = useState("");
  const [notes, setNotes] = useState("");
  const [dir, setDir] = useState("");
  const [lastRun, setLastRun] = useState<string | null>(null);
  const logging = status.state === "Logging" || !!stats?.logger;
  const canStart = status.state === "Ready" || status.state === "Error";
  const lg = stats?.logger;

  useEffect(() => {
    api.getDefaultLogDir().then(setDir).catch(() => {});
  }, []);

  const start = async () => {
    try {
      const s = await api.startLogging({ run_name: runName, notes, preset: activePreset, base_dir: dir || null });
      setLastRun(null);
      set({ lastError: null, stats: stats ? { ...stats, logger: s } : stats });
    } catch (e) {
      set({ lastError: String(e) });
    }
  };
  const stop = async () => {
    try {
      const m = await api.stopLogging();
      setLastRun(`${m.rows} rows · ${m.duration_s.toFixed(1)} s · ${m.average_sample_rate.toFixed(2)} rows/s · ${m.rows_dropped} dropped`);
    } catch (e) {
      set({ lastError: String(e) });
    }
  };

  const elapsed = lg ? lg.elapsed_s : 0;
  const p = stats?.poll;
  const ms = (v: number | null | undefined) => (v == null ? "—" : v.toFixed(1));

  return (
    <footer className="bottom">
      <div className="log-controls">
        <input placeholder="Run name (e.g. 3rd gear WOT)" value={runName} onChange={(e) => setRunName(e.target.value)} disabled={logging} />
        <input className="notes" placeholder="Notes" value={notes} onChange={(e) => setNotes(e.target.value)} disabled={logging} />
        {logging ? (
          <button className="danger" onClick={stop}>
            ■ Stop log
          </button>
        ) : (
          <button className="primary" onClick={start} disabled={!canStart}>
            ● Start log
          </button>
        )}
        <span className={`elapsed mono ${logging ? "rec" : ""}`}>{fmtDuration(elapsed)}</span>
      </div>
      <div className="log-path mono small" title={lg?.csv_path ?? dir}>
        {lg ? lg.csv_path : lastRun ? `last run: ${lastRun}` : `→ ${dir}`}
      </div>
      <div className="stats mono">
        <Stat k="req/s" v={stats ? stats.requests_per_s.toFixed(1) : "—"} />
        <Stat k="rows/s" v={stats ? stats.rows_per_s.toFixed(1) : "—"} />
        <Stat k="values/s" v={stats ? stats.samples_per_s.toFixed(0) : "—"} />
        <Stat k="RTT avg" v={ms(p?.rtt_avg_ms)} />
        <Stat k="min" v={ms(p?.rtt_min_ms)} />
        <Stat k="max" v={ms(p?.rtt_max_ms)} />
        <Stat k="failed" v={p?.failed ?? "—"} bad={!!p?.failed} />
        <Stat k="timeouts" v={p?.timeouts ?? "—"} bad={!!p?.timeouts} />
        <Stat k="checksum" v={p?.checksum_errors ?? "—"} bad={!!p?.checksum_errors} />
        <Stat k="dropped" v={stats ? stats.dropped_samples + (lg?.rows_dropped ?? 0) + stats.ui_dropped_rows : "—"} bad={!!stats && stats.dropped_samples + (lg?.rows_dropped ?? 0) > 0} />
        <Stat k="rows logged" v={lg?.rows_written ?? "—"} />
      </div>
      {(lastError || status.message) && (
        <div className={`notice ${lastError || status.state === "Error" ? "bad" : "warn"}`}>{lastError ?? status.message}</div>
      )}
    </footer>
  );
}

function Stat({ k, v, bad }: { k: string; v: string | number; bad?: boolean }) {
  return (
    <span className={`stat ${bad ? "bad" : ""}`}>
      <span className="muted">{k}</span> {v}
    </span>
  );
}
