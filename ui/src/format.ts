import type { AgentRow } from "./types";

/** "just now" under a minute, then 12m, 3h 05m, 2d. */
export function formatDuration(sinceMs: number, nowMs: number): string {
  const seconds = Math.max(0, Math.floor((nowMs - sinceMs) / 1000));
  if (seconds < 60) return "just now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
  return `${Math.floor(hours / 24)}d`;
}

/** Right-hand text of a row (spec section 11). */
export function stateText(row: AgentRow, nowMs: number): string {
  const dur = row.since === null ? null : formatDuration(row.since, nowMs);
  switch (row.status) {
    case "blocked":
      return dur ? `needs you · ${dur}` : "needs you";
    case "working":
      return dur ?? "working";
    case "done":
      return dur ? `done ${dur}` : "done";
    case "idle":
      return dur ? `idle ${dur}` : "idle";
    default:
      return "—";
  }
}

export function retryText(retryAtMs: number | null, nowMs: number): string {
  if (retryAtMs === null) return "";
  const seconds = Math.ceil((retryAtMs - nowMs) / 1000);
  return seconds > 0 ? `retrying ${seconds}s` : "retrying now";
}
