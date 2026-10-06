import { create } from "zustand";
import { api, pickFile } from "../lib/api";
import type { CalibrationSummary, TableData, TableInfo } from "../types/ms43";

const XDF_KEY = "logger43.xdfByVersion";

function loadXdfMap(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(XDF_KEY) ?? "{}");
  } catch {
    return {};
  }
}

export type StatMode = "values" | "hits" | "mean" | "delta";

interface CalState {
  summary: CalibrationSummary | null;
  tables: TableInfo[];
  selectedUid: number | null;
  table: TableData | null;
  /** XDF path per software version, e.g. { "430056": "...", "430069": "..." }. Remembered locally. */
  xdfByVersion: Record<string, string>;
  error: string | null;
  busy: boolean;
  query: string;
  overlayOnly: boolean;
  mode: StatMode;
  statChannel: string;
  trailSec: number;

  init: () => Promise<void>;
  loadBin: () => Promise<void>;
  chooseXdf: (version: string) => Promise<void>;
  unload: () => Promise<void>;
  select: (uid: number) => Promise<void>;
  set: (p: Partial<CalState>) => void;
}

export const useCal = create<CalState>((set, get) => ({
  summary: null,
  tables: [],
  selectedUid: null,
  table: null,
  xdfByVersion: loadXdfMap(),
  error: null,
  busy: false,
  query: "",
  overlayOnly: true,
  mode: "values",
  statChannel: "ignition_deg",
  trailSec: 5,

  init: async () => {
    const summary = await api.getCalibration().catch(() => null);
    if (summary) set({ summary, tables: await api.listTables() });
  },

  loadBin: async () => {
    const bin = await pickFile("Open MS43 calibration bin", ["bin", "ori", "BIN"]);
    if (!bin) return;
    set({ busy: true, error: null });
    try {
      const info = await api.inspectBin(bin);
      const version = info.software_version;
      if (!version) throw new Error("No MS43 software id (4300xx) found in this bin.");
      let xdf = get().xdfByVersion[version];
      if (!xdf) {
        xdf = (await pickFile(`Choose the XDF for ${version}`, ["xdf", "XDF"])) ?? "";
        if (!xdf) throw new Error(`No XDF chosen for ${version}.`);
        rememberXdf(version, xdf);
      }
      const summary = await api.loadCalibration(bin, xdf);
      set({ summary, tables: await api.listTables(), table: null, selectedUid: null });
    } catch (e) {
      set({ error: String(e) });
    } finally {
      set({ busy: false });
    }
  },

  chooseXdf: async (version) => {
    const xdf = await pickFile(`Choose the XDF for ${version}`, ["xdf", "XDF"]);
    if (xdf) rememberXdf(version, xdf);
  },

  unload: async () => {
    await api.unloadCalibration();
    set({ summary: null, tables: [], table: null, selectedUid: null });
  },

  select: async (uid) => {
    set({ selectedUid: uid, error: null });
    try {
      set({ table: await api.getTable(uid) });
    } catch (e) {
      set({ error: String(e), table: null });
    }
  },

  set: (p) => set(p),
}));

function rememberXdf(version: string, path: string) {
  const m = { ...useCal.getState().xdfByVersion, [version]: path };
  try {
    localStorage.setItem(XDF_KEY, JSON.stringify(m));
  } catch {
    /* storage unavailable: still works for this session */
  }
  useCal.setState({ xdfByVersion: m });
}
