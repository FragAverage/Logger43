// DEV-ONLY browser mock of the Tauri backend, used when the UI runs in a plain browser
// (`npm run dev`) for layout work. It is never bundled into the Tauri app: api.ts only imports it
// when `import.meta.env.DEV` and not inside Tauri. All data here is synthetic and labelled MOCK.
import catalog from "./devCatalog.json";
import type { ChannelInfo, ConnState, DebugRecord, Preset } from "../types/ms43";

type Handler = (payload: unknown) => void;
const handlers = new Map<string, Set<Handler>>();
const emit = (name: string, payload: unknown) => handlers.get(name)?.forEach((h) => h(payload));

const channels = catalog.channels as unknown as ChannelInfo[];
const presets = catalog.presets as unknown as Preset[];
let selected: string[] = [];
let state: ConnState = "Disconnected";
let timer: number | undefined;
let debug = false;
let logging: { started: number; rows: number } | null = null;
let t0 = 0;
let seq = 0;
let rows = 0;

// Warm-idle values of the real capture, used as baselines for the synthetic signals.
const BASE: Record<string, number> = {
  load_mg_stroke: 110, throttle_deg: 0.1, maf_g_s: 3.9, maf_kg_h: 14, ignition_deg: 8, injection_ms: 2.1,
  lambda_int_1: 0.985, lambda_int_2: 0.986, coolant_c: 92, iat_c: 40, oil_c: 96, radiator_out_c: 77,
  vanos_intake_actual: 118.5, vanos_intake_target: 118.5, vanos_exhaust_actual: -105.4, vanos_exhaust_target: -105.4,
  vanos_intake_actual_b90: 118.5, vanos_exhaust_actual_b90: -105.4, kl15_v: 13.8, battery_v: 13.6, baro_hpa: 988,
  idle_integrator_pct: -1.4, idle_actuator_pct: 38.5, knock_signal_2_v: 0.19, knock_signal_5_v: 0.1, fan_pct: 5,
  vehicle_speed_kph: 0, pedal_deg: 0,
};

function value(id: string, t: number): number {
  const ph = (t % 10) / 10; // 10 s "pull": idle -> WOT ramp
  const wot = ph > 0.2 && ph < 0.8;
  const rpm = wot ? 2000 + ((ph - 0.2) / 0.6) * 4500 : 750 + Math.sin(t * 3) * 15;
  const noise = (Math.random() - 0.5) * 0.02;
  switch (id) {
    case "rpm": return Math.round(rpm);
    case "throttle_deg": return wot ? 84 : 0.1;
    case "pedal_deg": return wot ? 90 : 0;
    case "load_mg_stroke": return wot ? 480 + noise * 400 : 110;
    case "maf_g_s": return (wot ? 480 : 110) * rpm * 3 / 60 / 1000 * (1 + noise);
    case "maf_kg_h": return ((wot ? 480 : 110) * rpm * 3 / 60 / 1000) * 3.6;
    case "ignition_deg": return wot ? 22 + (rpm / 6500) * 12 : 8;
    case "injection_ms": return wot ? 11 + noise * 5 : 2.1;
    case "vanos_intake_target": return wot ? 118 - (rpm / 6500) * 40 : 118.5;
    case "vanos_intake_actual": return value("vanos_intake_target", t - 0.15);
    case "vanos_exhaust_target": return wot ? -105 + (rpm / 6500) * 20 : -105.4;
    case "vanos_exhaust_actual": return value("vanos_exhaust_target", t - 0.2);
    case "lambda_int_1": return wot ? 1 : 0.985 + Math.sin(t * 4) * 0.02;
    case "knock_retard_deg": return wot && Math.random() < 0.05 ? -1.5 : 0;
    case "vehicle_speed_kph": return wot ? 40 + ((ph - 0.2) / 0.6) * 80 : 40;
    default:
      if (id.startsWith("ign_cyl")) return value("ignition_deg", t) - (Math.random() < 0.03 ? 1.5 : 0);
      return (BASE[id] ?? 0) * (1 + noise * 0.1);
  }
}

function setState(s: ConnState, message: string | null = null) {
  state = s;
  emit("connection_status", { state: s, message, port: "MOCK", simulated: true });
}

function plan() {
  const ids = selected.filter((id) => channels.some((c) => c.id === id));
  const unavailable = ids
    .filter((id) => channels.find((c) => c.id === id)?.verification === "Community")
    .map((id) => ({ id, reason: "MOCK: no known RAM address for DME software MOCK000" }));
  const cols = ids.filter((id) => !unavailable.some((u) => u.id === id));
  const requests = [
    { request: "0B03 measurements", frame: "12 05 0B 03 1F", priority: "Fast", every_n_cycles: 1, channels: cols.filter((c) => !c.includes("target") && !c.endsWith("_b90")) },
  ];
  if (cols.some((c) => c.includes("target") || c.endsWith("_b90")))
    requests.push({ request: "0B90 VANOS", frame: "12 05 0B 90 8C", priority: "Fast", every_n_cycles: 1, channels: cols.filter((c) => c.includes("target") || c.endsWith("_b90")) });
  return { columns: cols, requests, unavailable };
}

function tick() {
  const p = plan();
  const now = performance.now() / 1000 - t0;
  const ts = [now - 0.05, now];
  emit("sample_batch", { columns: p.columns, t: ts, values: p.columns.map((id) => ts.map((t) => value(id, t))) });
  rows += 2;
  if (logging) logging.rows += 2;
  if (debug) {
    const time = new Date().toISOString().replace("Z", "");
    const recs: DebugRecord[] = [
      { seq: seq++, time, dir: "TX", bytes: "12 05 0B 03 1F", note: "MOCK 0B03 measurements", rtt_ms: null, checksum_ok: null, decoded: [] },
      { seq: seq++, time, dir: "EC", bytes: "12 05 0B 03 1F", note: null, rtt_ms: null, checksum_ok: null, decoded: [] },
      { seq: seq++, time, dir: "RX", bytes: "12 2D A0 02 C3 00 00 … 86 C2", note: "MOCK RTT 70.1 ms, checksum OK", rtt_ms: 70.1, checksum_ok: true, decoded: [] },
      { seq: seq++, time, dir: "DEC", bytes: "", note: "MOCK decoded", rtt_ms: 70.1, checksum_ok: true, decoded: [{ id: "rpm", label: "Engine speed", unit: "rpm", raw_hex: "02C3", raw: 707, value: value("rpm", now) }] },
    ];
    emit("protocol_debug", recs);
  }
}

function stats() {
  const el = performance.now() / 1000 - t0;
  emit("logger_stats", {
    poll: { requests: rows, ok: rows, failed: 0, timeouts: 0, checksum_errors: 0, negative_responses: 0, other_errors: 0, discarded_bytes: 0, decoded_samples: rows * 10, elapsed_s: el, requests_per_s: 0, samples_per_s: 0, rtt_min_ms: 68.9, rtt_avg_ms: 70.4, rtt_max_ms: 74.2 },
    requests_per_s: 18.6, samples_per_s: 186, rows_per_s: 9.3, rows_total: rows, dropped_samples: 0, ui_dropped_rows: 0,
    logger: logging ? { run_name: "mock", run_dir: "~/Documents/Logger43/runs/mock", csv_path: "~/Documents/Logger43/runs/mock/run.csv", started_at: "", elapsed_s: (performance.now() - logging.started) / 1000, rows_written: logging.rows, rows_dropped: 0, bytes_written: 0 } : null,
    plan: plan(),
  });
}

let statsTimer: number | undefined;

export async function mockInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const r = (v: unknown) => v as T;
  switch (cmd) {
    case "list_serial_ports":
      return r([{ name: "COM4 (MOCK)", kind: "usb", vid: 0x0403, pid: 0x6001, manufacturer: "FTDI", product: "MOCK", serial_number: null, likely_kdcan: true }]);
    case "get_channels": return r(channels);
    case "get_presets": return r(presets);
    case "get_status": return r({ state, message: null, port: null, simulated: true });
    case "get_selected_channels": return r(selected);
    case "set_selected_channels":
      selected = (args?.ids as string[]) ?? [];
      return r(plan());
    case "connect_ecu": {
      t0 = performance.now() / 1000;
      const steps: ConnState[] = ["OpeningSerial", "InitialisingKLine", "StartingDiagnosticSession", "ReadingEcuId"];
      steps.forEach((s, i) => setTimeout(() => setState(s), i * 150));
      setTimeout(() => {
        emit("ecu_info", { ident: { bmw_part_number: "MOCK000", raw_ascii: "MOCK" }, layout_trust: "Unknown", description: null, raw_frame: "MOCK" });
        setState("Ready", "MOCK backend (browser dev mode) — synthetic data");
        timer = window.setInterval(tick, 100);
        statsTimer = window.setInterval(stats, 500);
      }, 700);
      return r(undefined);
    }
    case "disconnect_ecu":
      clearInterval(timer);
      clearInterval(statsTimer);
      logging = null;
      setState("Disconnected");
      return r(undefined);
    case "start_logging":
      logging = { started: performance.now(), rows: 0 };
      setState("Logging", "MOCK backend — nothing is written");
      return r({ run_name: "mock", run_dir: "", csv_path: "~/Documents/Logger43/runs/mock/run.csv", started_at: "", elapsed_s: 0, rows_written: 0, rows_dropped: 0, bytes_written: 0 });
    case "stop_logging": {
      const rowsLogged = logging?.rows ?? 0;
      logging = null;
      setState("Ready");
      return r({ run_name: "mock", rows: rowsLogged, rows_dropped: 0, duration_s: 1, average_sample_rate: 9.3, csv_file: "run.csv" });
    }
    case "set_debug":
      debug = !!args?.enabled;
      return r(undefined);
    case "get_default_log_dir": return r("~/Documents/Logger43/runs (MOCK)");
    default:
      return r(null);
  }
}

export async function mockListen(name: string, cb: (e: { payload: unknown }) => void): Promise<() => void> {
  const h: Handler = (p) => cb({ payload: p });
  if (!handlers.has(name)) handlers.set(name, new Set());
  handlers.get(name)!.add(h);
  return () => handlers.get(name)?.delete(h);
}
