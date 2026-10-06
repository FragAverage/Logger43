import type { ChannelInfo } from "../types/ms43";

/** Display decimals by unit: enough to see change, never noisy. */
export function decimals(c: ChannelInfo | undefined): number {
  if (!c) return 2;
  switch (c.unit) {
    case "rpm":
    case "km/h":
    case "hPa":
    case "°C":
    case "":
      return 0;
    case "mg/stroke":
    case "° crank":
    case "°":
    case "%":
    case "kg/h":
    case "deg":
      return 1;
    case "factor":
      return 3;
    default:
      return 2;
  }
}

export function fmt(c: ChannelInfo | undefined, v: number | null | undefined): string {
  if (v == null || !Number.isFinite(v)) return "—";
  return v.toFixed(decimals(c));
}

export const PALETTE = ["#4aa3ff", "#ffb347", "#3ccf8e", "#ef5f5f", "#c792ea", "#e5d14a", "#5fd7d7", "#ff7fbf"];

export function fmtDuration(s: number): string {
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`;
}
