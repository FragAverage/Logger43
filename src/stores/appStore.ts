import { create } from "zustand";
import { api } from "../lib/api";
import type {
  ChannelInfo,
  ConnectOptions,
  ConnectionStatus,
  DebugRecord,
  EcuInfo,
  LiveStats,
  PlanSummary,
  PortInfo,
  Preset,
  ProtocolErrorEvent,
} from "../types/ms43";

const DEBUG_KEEP = 3000;
export const SIMULATOR = "__simulator__";

export interface ChartConfig {
  id: string;
  title: string;
  channels: string[];
}

export const DEFAULT_CHARTS: ChartConfig[] = [
  { id: "c1", title: "RPM · throttle · load", channels: ["rpm", "throttle_deg", "load_mg_stroke"] },
  { id: "c2", title: "Ignition · knock", channels: ["ignition_deg", "knock_retard_deg", "ign_cyl_1", "ign_cyl_2", "ign_cyl_3", "ign_cyl_4", "ign_cyl_5", "ign_cyl_6"] },
  { id: "c3", title: "VANOS target vs actual", channels: ["vanos_intake_target", "vanos_intake_actual", "vanos_intake_actual_b90", "vanos_exhaust_target", "vanos_exhaust_actual", "vanos_exhaust_actual_b90"] },
  { id: "c4", title: "Lambda · injection", channels: ["lambda_int_1", "lambda_int_2", "injection_ms"] },
];

interface AppState {
  ports: PortInfo[];
  port: string;
  options: Omit<ConnectOptions, "port" | "simulate">;
  channels: ChannelInfo[];
  channelById: Record<string, ChannelInfo>;
  presets: Preset[];
  activePreset: string | null;
  selected: string[];
  plan: PlanSummary | null;
  status: ConnectionStatus;
  ecu: EcuInfo | null;
  stats: LiveStats | null;
  debug: DebugRecord[];
  debugCapture: boolean;
  errors: ProtocolErrorEvent[];
  page: "live" | "tables" | "debug";
  paused: boolean;
  windowSec: number;
  charts: ChartConfig[];
  lastError: string | null;

  init: () => Promise<void>;
  refreshPorts: () => Promise<void>;
  setPort: (p: string) => void;
  setOption: <K extends keyof AppState["options"]>(k: K, v: AppState["options"][K]) => void;
  connect: () => Promise<void>;
  disconnect: () => Promise<void>;
  select: (ids: string[], preset?: string | null) => Promise<void>;
  toggleChannel: (id: string) => Promise<void>;
  setDebugCapture: (on: boolean) => Promise<void>;
  pushDebug: (r: DebugRecord[]) => void;
  pushError: (e: ProtocolErrorEvent) => void;
  set: (p: Partial<AppState>) => void;
  setChartChannels: (chartId: string, channels: string[]) => void;
}

export const useApp = create<AppState>((set, get) => ({
  ports: [],
  port: SIMULATOR,
  options: { regen_ms: 100, timeout_ms: 2000, inter_byte_ms: 20, usb_latency_ms: 16, echo: true, dtr: false, rts: false },
  channels: [],
  channelById: {},
  presets: [],
  activePreset: null,
  selected: [],
  plan: null,
  status: { state: "Disconnected", message: null, port: null, simulated: false },
  ecu: null,
  stats: null,
  debug: [],
  debugCapture: false,
  errors: [],
  page: "live",
  paused: false,
  windowSec: 30,
  charts: DEFAULT_CHARTS,
  lastError: null,

  init: async () => {
    const [channels, presets, status, selected] = await Promise.all([
      api.getChannels(),
      api.getPresets(),
      api.getStatus(),
      api.getSelectedChannels(),
    ]);
    const channelById = Object.fromEntries(channels.map((c) => [c.id, c]));
    set({ channels, channelById, presets, status });
    await get().refreshPorts();
    if (selected.length) set({ selected });
    else await get().select(presets.find((p) => p.id === "power_run")?.channels ?? ["rpm"], "power_run");
  },

  refreshPorts: async () => {
    try {
      const ports = await api.listSerialPorts();
      const cur = get().port;
      const keep = cur === SIMULATOR || ports.some((p) => p.name === cur);
      set({ ports, port: keep ? cur : ports.find((p) => p.likely_kdcan)?.name ?? SIMULATOR });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  setPort: (port) => set({ port }),
  setOption: (k, v) => set({ options: { ...get().options, [k]: v } }),

  connect: async () => {
    const { port, options } = get();
    set({ lastError: null, ecu: null, errors: [] });
    try {
      await api.connect({ ...options, port: port === SIMULATOR ? "" : port, simulate: port === SIMULATOR });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  disconnect: async () => {
    await api.disconnect();
    set({ stats: null });
  },

  select: async (ids, preset = null) => {
    try {
      const plan = await api.setSelectedChannels(ids);
      set({ selected: ids, plan, activePreset: preset, lastError: null });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },

  toggleChannel: async (id) => {
    const sel = get().selected;
    await get().select(sel.includes(id) ? sel.filter((s) => s !== id) : [...sel, id], null);
  },

  setDebugCapture: async (on) => {
    await api.setDebug(on);
    set({ debugCapture: on });
  },

  pushDebug: (r) => {
    const next = get().debug.concat(r);
    set({ debug: next.length > DEBUG_KEEP ? next.slice(next.length - DEBUG_KEEP) : next });
  },

  pushError: (e) => {
    const errors = [...get().errors.slice(-199), e];
    set({ errors, lastError: e.fatal ? e.message : get().lastError });
  },

  set: (p) => set(p),

  setChartChannels: (chartId, channels) =>
    set({ charts: get().charts.map((c) => (c.id === chartId ? { ...c, channels } : c)) }),
}));

export const isBusy = (s: ConnectionStatus["state"]) =>
  s === "OpeningSerial" || s === "InitialisingKLine" || s === "StartingDiagnosticSession" || s === "ReadingEcuId";
export const isOnline = (s: ConnectionStatus["state"]) => s === "Ready" || s === "Logging" || s === "Error";
