import { describe, expect, it } from "vitest";
import { formatDuration, retryText, stateText } from "../format";
import type { AgentRow } from "../types";

const row = (status: AgentRow["status"], since: number | null): AgentRow => ({
  server_id: "local",
  server_label: "local",
  pane_id: "w1:p1",
  workspace_label: "ws",
  agent_kind: "claude",
  name: "x",
  status,
  since,
  flash_seq: 0,
});

const NOW = 1_000_000_000_000;
const s = (n: number) => n * 1000;

describe("formatDuration", () => {
  it("buckets by magnitude", () => {
    expect(formatDuration(NOW - s(5), NOW)).toBe("just now");
    expect(formatDuration(NOW - s(59), NOW)).toBe("just now");
    expect(formatDuration(NOW - s(60), NOW)).toBe("1m");
    expect(formatDuration(NOW - s(12 * 60 + 30), NOW)).toBe("12m");
    expect(formatDuration(NOW - s(3 * 3600 + 5 * 60), NOW)).toBe("3h 05m");
    expect(formatDuration(NOW - s(26 * 3600), NOW)).toBe("1d");
    expect(formatDuration(NOW + s(10), NOW)).toBe("just now");
  });
});

describe("stateText", () => {
  it("per status, with and without a duration", () => {
    expect(stateText(row("blocked", NOW - s(120)), NOW)).toBe("needs you · 2m");
    expect(stateText(row("blocked", null), NOW)).toBe("needs you");
    expect(stateText(row("working", NOW - s(240)), NOW)).toBe("4m");
    expect(stateText(row("working", null), NOW)).toBe("working");
    expect(stateText(row("done", NOW - s(720)), NOW)).toBe("done 12m");
    expect(stateText(row("done", null), NOW)).toBe("done");
    expect(stateText(row("idle", NOW - s(1500)), NOW)).toBe("idle 25m");
    expect(stateText(row("idle", null), NOW)).toBe("idle");
    expect(stateText(row("unknown", NOW), NOW)).toBe("—");
  });
});

describe("retryText", () => {
  it("counts down to the retry", () => {
    expect(retryText(NOW + s(30), NOW)).toBe("retrying 30s");
    expect(retryText(NOW - s(1), NOW)).toBe("retrying now");
  });
});
