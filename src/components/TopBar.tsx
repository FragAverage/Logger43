import { useState } from "react";
import { isBusy, SIMULATOR, useApp } from "../stores/appStore";
import type { ConnState } from "../types/ms43";

const STATE_LABEL: Record<ConnState, string> = {
  Disconnected: "Disconnected",
  OpeningSerial: "Opening serial…",
  InitialisingKLine: "Initialising K-line…",
  StartingDiagnosticSession: "Starting diagnostic session…",
  ReadingEcuId: "Reading ECU ID…",
  Ready: "Ready",
  Logging: "Logging",
  Error: "Error",
};

export function TopBar() {
  const { ports, port, setPort, refreshPorts, status, connect, disconnect, ecu, stats, options, setOption, page, set } = useApp();
  const [showSettings, setShowSettings] = useState(false);
  const busy = isBusy(status.state);
  const disconnected = status.state === "Disconnected";
  const trust = ecu?.layout_trust;

  return (
    <header className="topbar">
      <span className="brand">LOGGER43</span>

      <select value={port} onChange={(e) => setPort(e.target.value)} disabled={!disconnected} title="Serial port">
        <option value={SIMULATOR}>Simulator (no hardware)</option>
        {ports.map((p) => (
          <option key={p.name} value={p.name}>
            {p.name}
            {p.likely_kdcan ? " — FTDI K+DCAN?" : ""}
          </option>
        ))}
      </select>
      <button onClick={refreshPorts} disabled={!disconnected} title="Refresh ports">
        ⟳
      </button>
      {disconnected ? (
        <button className="primary" onClick={connect}>
          Connect
        </button>
      ) : (
        <button onClick={disconnect} disabled={busy}>
          Disconnect
        </button>
      )}
      <button className={showSettings ? "active" : ""} onClick={() => setShowSettings(!showSettings)} title="Link settings">
        ⚙
      </button>

      <span className={`state state-${status.state}`} title={status.message ?? ""}>
        <i />
        {STATE_LABEL[status.state]}
        {status.simulated && <em> · SIM</em>}
      </span>

      <span className="ecu" title={ecu ? `${ecu.ident.raw_ascii}\n${ecu.raw_frame}` : ""}>
        {ecu ? (
          <>
            DME <b className="mono">{ecu.ident.bmw_part_number ?? "?"}</b>
            <span className={`tag ${trust === "KnownMs43" ? "ok" : trust === "OtherDme" ? "bad" : "warn"}`}>
              {trust === "KnownMs43" ? "MS43" : trust === "OtherDme" ? "not MS43" : "unknown SW"}
            </span>
          </>
        ) : (
          <span className="muted">no ECU</span>
        )}
      </span>

      <span className="rate mono" title="poller cycles/s · decoded values/s · requests/s">
        <b>{stats ? stats.rows_per_s.toFixed(1) : "—"}</b> rows/s
        <span className="muted"> · {stats ? stats.samples_per_s.toFixed(0) : "—"} val/s · {stats ? stats.requests_per_s.toFixed(1) : "—"} req/s</span>
      </span>

      <nav className="tabs">
        <button className={page === "live" ? "active" : ""} onClick={() => set({ page: "live" })}>
          Live
        </button>
        <button className={page === "debug" ? "active" : ""} onClick={() => set({ page: "debug" })}>
          Raw debug
        </button>
      </nav>

      {showSettings && (
        <div className="settings" onMouseLeave={() => setShowSettings(false)}>
          <div className="muted small">Applied on next connect. Defaults from BMW MS430DS0 / EdiabasLib.</div>
          <NumField label="Regen (gap after response) ms" value={options.regen_ms} onChange={(v) => setOption("regen_ms", v)} hint="BMW 100. Lower after Stage 6." />
          <NumField label="Response timeout ms" value={options.timeout_ms} onChange={(v) => setOption("timeout_ms", v)} hint="BMW 2000" />
          <NumField label="Inter-byte timeout ms" value={options.inter_byte_ms} onChange={(v) => setOption("inter_byte_ms", v)} hint="BMW 20" />
          <NumField label="USB latency allowance ms" value={options.usb_latency_ms} onChange={(v) => setOption("usb_latency_ms", v)} hint="= FTDI latency timer" />
          <label className="check">
            <input type="checkbox" checked={options.echo} onChange={(e) => setOption("echo", e.target.checked)} /> Cable echoes K-line (K+DCAN: yes)
          </label>
          <label className="check">
            <input type="checkbox" checked={options.dtr} onChange={(e) => setOption("dtr", e.target.checked)} /> Assert DTR
          </label>
          <label className="check">
            <input type="checkbox" checked={options.rts} onChange={(e) => setOption("rts", e.target.checked)} /> Assert RTS
          </label>
        </div>
      )}
    </header>
  );
}

function NumField({ label, value, onChange, hint }: { label: string; value: number; onChange: (v: number) => void; hint: string }) {
  return (
    <label className="numfield">
      <span>{label}</span>
      <input type="number" min={0} value={value} onChange={(e) => onChange(Math.max(0, Number(e.target.value) || 0))} />
      <span className="muted small">{hint}</span>
    </label>
  );
}
