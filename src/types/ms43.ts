// Mirrors serde output of ms43-core. Keep in sync with crates/ms43-core/src/{serial,channels,engine,plan,logger}.rs

export interface PortInfo {
  name: string;
  kind: string;
  vid: number | null;
  pid: number | null;
  manufacturer: string | null;
  product: string | null;
  serial_number: string | null;
  likely_kdcan: boolean;
}

export type DataType = "U8" | "S8" | "U16Le" | "U16Be" | "S16Le" | "S16Be";
export type Priority = "Fast" | "Normal" | "Slow";
export type Verification = "CrossChecked" | "Sgbd" | "Community";

export interface ChannelInfo {
  id: string;
  label: string;
  unit: string;
  column: string;
  factor: number;
  offset: number;
  priority: Priority;
  verification: Verification;
  sources: string;
  note: string | null;
  group: string;
  source_text: string;
}

export interface Preset {
  id: string;
  name: string;
  description: string;
  channels: string[];
}

export interface ConnectOptions {
  port: string;
  simulate: boolean;
  regen_ms: number;
  timeout_ms: number;
  inter_byte_ms: number;
  usb_latency_ms: number;
  echo: boolean;
  dtr: boolean;
  rts: boolean;
}

export type ConnState =
  | "Disconnected"
  | "OpeningSerial"
  | "InitialisingKLine"
  | "StartingDiagnosticSession"
  | "ReadingEcuId"
  | "Ready"
  | "Logging"
  | "Error";

export interface ConnectionStatus {
  state: ConnState;
  message: string | null;
  port: string | null;
  simulated: boolean;
}

export interface EcuIdent {
  bmw_part_number: string | null;
  hardware_number: string | null;
  coding_index: string | null;
  diag_index: string | null;
  bus_index: string | null;
  production_week: string | null;
  production_year: string | null;
  supplier_number: string | null;
  software_number: string | null;
  change_index: string | null;
  production_number: string | null;
  raw_ascii: string;
}

export type LayoutTrust = "KnownMs43" | "OtherDme" | "Unknown";

export interface EcuInfo {
  ident: EcuIdent;
  layout_trust: LayoutTrust;
  description: string | null;
  raw_frame: string;
}

export interface SampleBatch {
  columns: string[];
  t: number[];
  values: (number | null)[][];
}

export interface StatsSnapshot {
  requests: number;
  ok: number;
  failed: number;
  timeouts: number;
  checksum_errors: number;
  negative_responses: number;
  other_errors: number;
  discarded_bytes: number;
  decoded_samples: number;
  elapsed_s: number;
  requests_per_s: number;
  samples_per_s: number;
  rtt_min_ms: number | null;
  rtt_avg_ms: number | null;
  rtt_max_ms: number | null;
}

export interface LoggerStatus {
  run_name: string;
  run_dir: string;
  csv_path: string;
  started_at: string;
  elapsed_s: number;
  rows_written: number;
  rows_dropped: number;
  bytes_written: number;
}

export interface JobSummary {
  request: string;
  frame: string;
  priority: Priority;
  every_n_cycles: number;
  channels: string[];
}

export interface PlanSummary {
  columns: string[];
  requests: JobSummary[];
  unavailable: { id: string; reason: string }[];
}

export interface LiveStats {
  poll: StatsSnapshot;
  requests_per_s: number;
  samples_per_s: number;
  rows_per_s: number;
  rows_total: number;
  dropped_samples: number;
  ui_dropped_rows: number;
  logger: LoggerStatus | null;
  plan: PlanSummary | null;
}

export interface DebugValue {
  id: string;
  label: string;
  unit: string;
  raw_hex: string;
  raw: number;
  value: number;
}

export interface DebugRecord {
  seq: number;
  time: string;
  dir: "TX" | "EC" | "RX" | "XX" | "ER" | "DEC";
  bytes: string;
  note: string | null;
  rtt_ms: number | null;
  checksum_ok: boolean | null;
  decoded: DebugValue[];
}

export interface ProtocolErrorEvent {
  time: string;
  message: string;
  fatal: boolean;
}

export interface RunMeta {
  run_name: string;
  rows: number;
  rows_dropped: number;
  duration_s: number;
  average_sample_rate: number;
  csv_file: string;
}
