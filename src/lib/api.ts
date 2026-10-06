// Typed wrappers around Tauri commands and events. The only place that knows command names.
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ChannelInfo,
  ConnectOptions,
  ConnectionStatus,
  DebugRecord,
  EcuInfo,
  LiveStats,
  LoggerStatus,
  PlanSummary,
  PortInfo,
  Preset,
  ProtocolErrorEvent,
  RunMeta,
  SampleBatch,
} from "../types/ms43";

const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

// Browser dev mode only (`npm run dev` outside Tauri): synthetic backend for UI work.
// `import.meta.env.DEV` is a literal `false` in production builds, so the mock is never bundled.
async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (import.meta.env.DEV) {
    if (!inTauri()) return (await import("./devMock")).mockInvoke<T>(cmd, args);
  }
  return tauriInvoke<T>(cmd, args);
}

async function listen(name: string, cb: (e: { payload: unknown }) => void): Promise<UnlistenFn> {
  if (import.meta.env.DEV) {
    if (!inTauri()) return (await import("./devMock")).mockListen(name, cb);
  }
  return tauriListen(name, cb);
}

export const api = {
  listSerialPorts: () => invoke<PortInfo[]>("list_serial_ports"),
  getChannels: () => invoke<ChannelInfo[]>("get_channels"),
  getPresets: () => invoke<Preset[]>("get_presets"),
  connect: (options: ConnectOptions) => invoke<void>("connect_ecu", { options }),
  disconnect: () => invoke<void>("disconnect_ecu"),
  getStatus: () => invoke<ConnectionStatus>("get_status"),
  getEcuInfo: () => invoke<EcuInfo | null>("get_ecu_info"),
  setSelectedChannels: (ids: string[]) => invoke<PlanSummary>("set_selected_channels", { ids }),
  getSelectedChannels: () => invoke<string[]>("get_selected_channels"),
  startLogging: (args: { run_name: string; notes: string; preset: string | null; base_dir: string | null }) =>
    invoke<LoggerStatus>("start_logging", { args }),
  stopLogging: () => invoke<RunMeta>("stop_logging"),
  getLoggerStats: () => invoke<LiveStats | null>("get_logger_stats"),
  getRecentSamples: (seconds: number) => invoke<SampleBatch | null>("get_recent_samples", { seconds }),
  setDebug: (enabled: boolean) => invoke<void>("set_debug", { enabled }),
  startRawRecording: () => invoke<string>("start_raw_recording"),
  stopRawRecording: () => invoke<string | null>("stop_raw_recording"),
  saveDebugHistory: () => invoke<string>("save_debug_history"),
  getDefaultLogDir: () => invoke<string>("get_default_log_dir"),
};

export interface EngineListeners {
  connection_status: (p: ConnectionStatus) => void;
  ecu_info: (p: EcuInfo) => void;
  sample_batch: (p: SampleBatch) => void;
  logger_stats: (p: LiveStats) => void;
  protocol_debug: (p: DebugRecord[]) => void;
  protocol_error: (p: ProtocolErrorEvent) => void;
}

export async function listenEngine(h: EngineListeners): Promise<UnlistenFn> {
  const offs = await Promise.all(
    (Object.keys(h) as (keyof EngineListeners)[]).map((name) =>
      listen(name, (e) => (h[name] as (p: unknown) => void)(e.payload)),
    ),
  );
  return () => offs.forEach((off) => off());
}
