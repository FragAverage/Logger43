import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import { useApp } from "../stores/appStore";
import type { DebugRecord } from "../types/ms43";

/** Raw protocol view: every TX / echo / RX / discarded byte, errors, and decoded values with raw hex. */
export function DebugPage() {
  const { debug, debugCapture, setDebugCapture, set, errors, plan } = useApp();
  const [filter, setFilter] = useState("");
  const [errorsOnly, setErrorsOnly] = useState(false);
  const [follow, setFollow] = useState(true);
  const [recording, setRecording] = useState<string | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const tail = useRef<HTMLDivElement>(null);

  const rows = useMemo(() => {
    const f = filter.trim().toLowerCase();
    let r: DebugRecord[] = debug;
    if (errorsOnly) r = r.filter((d) => d.dir === "ER" || d.dir === "XX" || d.checksum_ok === false);
    if (f)
      r = r.filter(
        (d) =>
          d.bytes.toLowerCase().includes(f) ||
          (d.note ?? "").toLowerCase().includes(f) ||
          d.decoded.some((v) => v.id.includes(f)),
      );
    return r.slice(-800);
  }, [debug, filter, errorsOnly]);

  useEffect(() => {
    if (follow) tail.current?.scrollIntoView({ block: "end" });
  }, [rows, follow]);

  const toggleRecord = async () => {
    try {
      if (recording) {
        const p = await api.stopRawRecording();
        setMsg(`Raw session saved: ${p ?? recording}`);
        setRecording(null);
      } else {
        setRecording(await api.startRawRecording());
      }
    } catch (e) {
      setMsg(String(e));
    }
  };
  const saveHistory = async () => {
    try {
      setMsg(`Saved last 5000 records: ${await api.saveDebugHistory()}`);
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <div className="debug">
      <div className="debug-bar">
        <label className="check">
          <input type="checkbox" checked={debugCapture} onChange={(e) => setDebugCapture(e.target.checked)} /> Live capture
        </label>
        <button className={recording ? "danger" : ""} onClick={toggleRecord}>
          {recording ? "■ Stop recording" : "● Record raw session to file"}
        </button>
        <button onClick={saveHistory} title="The backend always keeps the last 5000 records">
          Save last 5000
        </button>
        <button onClick={() => set({ debug: [] })}>Clear view</button>
        <input placeholder="filter: bytes, note, channel id" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <label className="check">
          <input type="checkbox" checked={errorsOnly} onChange={(e) => setErrorsOnly(e.target.checked)} /> problems only
        </label>
        <label className="check">
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> follow
        </label>
      </div>
      {(msg || recording) && <div className="notice warn mono small">{recording ? `recording → ${recording}` : msg}</div>}
      {!debugCapture && <div className="notice warn">Live capture is off. Turn it on to stream frames (the backend still keeps the last 5000).</div>}

      <div className="debug-split">
        <div className="frames">
          <table className="mono">
            <thead>
              <tr>
                <th>time</th>
                <th>dir</th>
                <th>frame / decoded</th>
                <th>RTT</th>
                <th>chk</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((d) => (
                <tr key={d.seq} className={`dir-${d.dir}`}>
                  <td className="nowrap">{d.time.slice(11)}</td>
                  <td>{d.dir}</td>
                  <td>
                    {d.bytes && <div className="bytes">{d.bytes}</div>}
                    {d.note && <div className="note">{d.note}</div>}
                    {d.decoded.map((v) => (
                      <div key={v.id} className="decoded">
                        {v.id} raw=0x{v.raw_hex} ({v.raw}) → <b>{v.value.toFixed(3)}</b> {v.unit}
                      </div>
                    ))}
                  </td>
                  <td className="nowrap">{d.rtt_ms != null ? `${d.rtt_ms.toFixed(1)} ms` : ""}</td>
                  <td>{d.checksum_ok == null ? "" : d.checksum_ok ? "ok" : "BAD"}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div ref={tail} />
        </div>
        <div className="debug-side">
          <h2>Request plan</h2>
          {plan?.requests.map((r) => (
            <div key={r.frame} className="small">
              <div className="mono">{r.frame}</div>
              <div className="muted">
                {r.request} · {r.priority} · every {r.every_n_cycles} · {r.channels.join(", ")}
              </div>
            </div>
          ))}
          {plan?.unavailable.map((u) => (
            <div key={u.id} className="small bad">
              {u.id}: {u.reason}
            </div>
          ))}
          <h2>Errors ({errors.length})</h2>
          <div className="errors mono small">
            {errors
              .slice()
              .reverse()
              .map((e, i) => (
                <div key={i} className={e.fatal ? "bad" : ""}>
                  {e.time.slice(11)} {e.message}
                </div>
              ))}
          </div>
        </div>
      </div>
    </div>
  );
}
