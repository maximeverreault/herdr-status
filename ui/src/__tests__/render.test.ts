import { beforeEach, describe, expect, it } from "vitest";
import { renderHeader, renderList } from "../render";
import { EMPTY_LIST, type AgentList, type AgentRow } from "../types";

const row = (over: Partial<AgentRow>): AgentRow => ({
  server_id: "local",
  server_label: "local",
  pane_id: "w1:p1",
  workspace_label: "swat",
  agent_kind: "claude",
  name: "reviewer",
  status: "working",
  since: null,
  flash_seq: 0,
  ...over,
});

const NOW = 1_000_000_000_000;

function list(over: Partial<AgentList>): AgentList {
  return { ...EMPTY_LIST, servers: [{ id: "local", label: "local", kind: "local", connection: { state: "online" }, transport: "unix socket /x" }], ...over };
}

let header: HTMLElement;
let main: HTMLElement;
let drawer: HTMLElement;

beforeEach(() => {
  document.body.innerHTML = `<header id="h"></header><main id="l"></main>`;
  header = document.getElementById("h")!;
  main = document.getElementById("l")!;
  drawer = document.createElement("div");
  drawer.className = "drawer";
});

describe("renderHeader", () => {
  it("shows one chip per server with its connection class and tooltip", () => {
    const l = list({
      servers: [
        { id: "local", label: "local", kind: "local", connection: { state: "online" }, transport: "unix socket /x" },
        { id: "box", label: "box", kind: "remote", connection: { state: "offline", reason: "Unreachable", retry_at: NOW + 30000 }, transport: "ssh box (posix)" },
        { id: "wsl", label: "wsl", kind: "wsl", connection: { state: "connecting" }, transport: "" },
      ],
    });
    renderHeader(header, l, NOW);
    const chips = header.querySelectorAll(".chip");
    expect(chips.length).toBe(3);
    expect(chips[0]!.className).toContain("online");
    expect(chips[0]!.getAttribute("title")).toBe("unix socket /x");
    expect(chips[1]!.className).toContain("offline");
    expect(chips[1]!.getAttribute("title")).toContain("Unreachable");
    expect(chips[1]!.getAttribute("title")).toContain("retrying 30s");
    expect(chips[2]!.className).toContain("connecting");
    expect(header.querySelector(".brand")!.textContent).toBe("herdr");
  });
});

describe("renderList", () => {
  it("renders rows in list order with dot, name, meta and state text", () => {
    const l = list({
      rows: [
        row({ pane_id: "w1:p2", name: "reviewer", status: "blocked", since: NOW - 120000, agent_kind: "codex", server_label: "winbox", server_id: "winbox" }),
        row({ pane_id: "w1:p1", name: "db migration", status: "working", since: NOW - 240000 }),
      ],
    });
    renderList(main, l, NOW, { openKey: null, drawer, onRowClick: () => {} });
    const rows = main.querySelectorAll(".row");
    expect(rows.length).toBe(2);
    expect(rows[0]!.className).toContain("blocked");
    expect(rows[0]!.querySelector(".name")!.textContent).toBe("reviewer");
    expect(rows[0]!.querySelector(".meta")!.textContent).toBe("winbox · swat · codex");
    expect(rows[0]!.querySelector(".state")!.textContent).toBe("needs you · 2m");
    expect(rows[1]!.querySelector(".state")!.textContent).toBe("4m");
    expect(rows[1]!.getAttribute("data-key")).toBe("local|w1:p1");
  });

  it("reuses elements across renders and flashes when flash_seq changes", () => {
    const first = list({ rows: [row({ flash_seq: 0 })] });
    renderList(main, first, NOW, { openKey: null, drawer, onRowClick: () => {} });
    const el = main.querySelector(".row")!;
    expect(el.classList.contains("flash")).toBe(false);
    const second = list({ rows: [row({ flash_seq: 1, status: "blocked" })] });
    renderList(main, second, NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".row")).toBe(el);
    expect(el.classList.contains("flash")).toBe(true);
    expect(el.classList.contains("blocked")).toBe(true);
  });

  it("places the drawer after the open row and removes rows that are gone", () => {
    const l = list({ rows: [row({ pane_id: "a" }), row({ pane_id: "b" })] });
    renderList(main, l, NOW, { openKey: "local|b", drawer, onRowClick: () => {} });
    const rowB = main.querySelector('[data-key="local|b"]')!;
    expect(rowB.nextElementSibling).toBe(drawer);
    expect(rowB.classList.contains("open")).toBe(true);
    renderList(main, list({ rows: [row({ pane_id: "a" })] }), NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector('[data-key="local|b"]')).toBeNull();
    expect(drawer.isConnected).toBe(false);
  });

  it("renders offline groups dimmed with reason and retry, after online rows", () => {
    const l = list({
      rows: [row({ pane_id: "a" })],
      offline: [{ server_id: "box", label: "build-box", reason: "Unreachable", retry_at: NOW + 30000, rows: [row({ server_id: "box", server_label: "build-box", pane_id: "z", status: "idle" })] }],
    });
    renderList(main, l, NOW, { openKey: null, drawer, onRowClick: () => {} });
    const group = main.querySelector(".group")!;
    expect(group.querySelector(".group-label")!.textContent).toBe("build-box");
    expect(group.querySelector(".group-status")!.textContent).toBe("Unreachable · retrying 30s");
    const dimmed = main.querySelectorAll(".row.dim");
    expect(dimmed.length).toBe(1);
    expect(main.children[0]!.classList.contains("row")).toBe(true);
    expect(main.children[1]!.classList.contains("group")).toBe(true);
  });

  it("shows empty states", () => {
    renderList(main, list({}), NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".empty")!.textContent).toBe("No agents running");
    const down = list({ servers: [{ id: "local", label: "local", kind: "local", connection: { state: "offline", reason: "herdr is not running", retry_at: NOW }, transport: "" }] });
    renderList(main, down, NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".empty")!.textContent).toBe("herdr is not running");
  });

  it("calls onRowClick with the row", () => {
    let clicked: AgentRow | null = null;
    renderList(main, list({ rows: [row({})] }), NOW, { openKey: null, drawer, onRowClick: (r) => (clicked = r) });
    (main.querySelector(".row") as HTMLElement).click();
    expect(clicked!.pane_id).toBe("w1:p1");
  });
});

describe("reconnecting group", () => {
  it("renders_reconnecting_group_without_retry_text", () => {
    const l = list({
      offline: [{ server_id: "box", label: "build-box", reason: "Reconnecting…", retry_at: null, rows: [row({ server_id: "box", server_label: "build-box", pane_id: "z", status: "idle" })] }],
    });
    renderList(main, l, NOW, { openKey: null, drawer, onRowClick: () => {} });
    expect(main.querySelector(".group-status")!.textContent).toBe("Reconnecting…");
  });
});
