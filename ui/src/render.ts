import { retryText, stateText } from "./format";
import { rowKey, type AgentList, type AgentRow, type OfflineGroup } from "./types";

export interface RenderOptions {
  openKey: string | null;
  drawer: HTMLElement;
  onRowClick: (row: AgentRow) => void;
}

const reducedMotion = (): boolean =>
  typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;

export function renderHeader(root: HTMLElement, list: AgentList, nowMs: number): void {
  let brand = root.querySelector<HTMLElement>(":scope > .brand");
  if (!brand) {
    brand = document.createElement("span");
    brand.className = "brand";
    brand.textContent = "herdr";
    root.prepend(brand);
  }
  let chips = root.querySelector<HTMLElement>(":scope > .chips");
  if (!chips) {
    chips = document.createElement("div");
    chips.className = "chips";
    root.appendChild(chips);
  }
  const existing = new Map<string, HTMLElement>();
  chips.querySelectorAll<HTMLElement>(":scope > .chip").forEach((el) => existing.set(el.dataset.server!, el));
  const ordered: HTMLElement[] = [];
  for (const server of list.servers) {
    let chip = existing.get(server.id);
    if (!chip) {
      chip = document.createElement("span");
      chip.dataset.server = server.id;
      chip.innerHTML = `<i></i><span class="chip-label"></span>`;
    }
    chip.className = `chip ${server.connection.state}`;
    chip.querySelector(".chip-label")!.textContent = server.label;
    chip.title =
      server.connection.state === "offline"
        ? `${server.connection.reason} · ${retryText(server.connection.retry_at, nowMs)}${server.transport ? ` · ${server.transport}` : ""}`
        : server.transport || server.connection.state;
    ordered.push(chip);
  }
  syncChildren(chips, ordered);
}

/** Make `root`'s children exactly `ordered`, touching only nodes that are out of place. */
function syncChildren(root: Node, ordered: Node[]): void {
  ordered.forEach((node, i) => {
    const at = root.childNodes[i];
    if (at !== node) root.insertBefore(node, at ?? null);
  });
  while (root.childNodes.length > ordered.length) root.lastChild!.remove();
}

/** Row elements carry their latest row so click handlers never see stale data. */
interface RowElement extends HTMLElement {
  row?: AgentRow;
}

function buildRow(row: AgentRow, onClick: (row: AgentRow) => void): RowElement {
  const el = document.createElement("div") as RowElement;
  el.className = "row enter";
  el.dataset.key = rowKey(row);
  el.innerHTML = `<div class="dot"></div><div class="text"><div class="name"></div><div class="meta"></div></div><div class="state"></div>`;
  el.addEventListener("click", () => {
    if (el.row) onClick(el.row);
  });
  el.addEventListener("animationend", (e) => {
    if (e.target !== el) return;
    if (e.animationName.startsWith("flash")) el.classList.remove("flash");
    else if (e.animationName.startsWith("enter")) el.classList.remove("enter");
  });
  return el;
}

function fillRow(el: RowElement, row: AgentRow, nowMs: number, open: boolean, dim: boolean): void {
  el.row = row;
  const keep = ["flash", "enter"].filter((c) => el.classList.contains(c)).join(" ");
  el.className = `row ${row.status}${open ? " open" : ""}${dim ? " dim" : ""}${keep ? ` ${keep}` : ""}`;
  el.querySelector(".name")!.textContent = row.name;
  const meta = [row.server_label, row.workspace_label, row.agent_kind].filter((p) => p && p.length > 0).join(" · ");
  el.querySelector(".meta")!.textContent = meta;
  el.querySelector(".state")!.textContent = stateText(row, nowMs);
  const previous = el.dataset.flashSeq;
  if (previous !== undefined && Number(previous) !== row.flash_seq) {
    el.classList.remove("flash");
    void (el as HTMLElement).offsetWidth; // restart the animation
    el.classList.add("flash");
  }
  el.dataset.flashSeq = String(row.flash_seq);
}

function fillGroup(el: HTMLElement, group: OfflineGroup, nowMs: number): void {
  el.querySelector(".group-label")!.textContent = group.label;
  el.querySelector(".group-status")!.textContent = group.retry_at === null ? group.reason : `${group.reason} · ${retryText(group.retry_at, nowMs)}`;
}

function buildGroup(group: OfflineGroup, nowMs: number): HTMLElement {
  const el = document.createElement("div");
  el.className = "group";
  el.dataset.server = group.server_id;
  el.innerHTML = `<div class="group-line"><span class="group-label"></span><span class="group-status"></span></div>`;
  fillGroup(el, group, nowMs);
  return el;
}

function emptyText(list: AgentList): string | null {
  if (list.rows.length > 0 || list.offline.some((g) => g.rows.length > 0)) return null;
  if (list.servers.some((s) => s.connection.state === "online")) return "No agents running";
  if (list.servers.length === 0) return "Connecting…";
  if (list.servers.some((s) => s.connection.state === "connecting")) return "Connecting…";
  const only = list.servers.length === 1 ? list.servers[0]! : null;
  if (only && only.id === "local") return "herdr is not running";
  return "No servers reachable";
}

/**
 * Re-render the list. Existing row elements are reused (keyed), the drawer is
 * moved after the open row, offline groups follow the online rows, and rows
 * that moved get a short FLIP transition.
 */
export function renderList(root: HTMLElement, list: AgentList, nowMs: number, opts: RenderOptions): void {
  const existing = new Map<string, RowElement>();
  root.querySelectorAll<RowElement>(".row[data-key]").forEach((el) => existing.set(el.dataset.key!, el));
  const groups = new Map<string, HTMLElement>();
  root.querySelectorAll<HTMLElement>(":scope > .group").forEach((el) => groups.set(el.dataset.server!, el));
  const before = new Map<string, DOMRect>();
  if (!reducedMotion()) existing.forEach((el, key) => before.set(key, el.getBoundingClientRect()));

  const ordered: Node[] = [];
  const seen = new Set<string>();
  const place = (row: AgentRow, dim: boolean) => {
    const key = rowKey(row);
    let el = existing.get(key);
    if (!el) el = buildRow(row, opts.onRowClick);
    const open = opts.openKey === key;
    fillRow(el, row, nowMs, open, dim);
    seen.add(key);
    ordered.push(el);
    if (open) ordered.push(opts.drawer);
  };

  for (const row of list.rows) place(row, false);
  for (const group of list.offline) {
    let groupEl = groups.get(group.server_id);
    if (groupEl) fillGroup(groupEl, group, nowMs);
    else groupEl = buildGroup(group, nowMs);
    ordered.push(groupEl);
    for (const row of group.rows) place(row, true);
  }
  const empty = emptyText(list);
  if (empty) {
    const el = root.querySelector<HTMLElement>(":scope > .empty") ?? document.createElement("div");
    el.className = "empty";
    el.textContent = empty;
    ordered.push(el);
  }
  const active = document.activeElement;
  const peek = opts.drawer.querySelector<HTMLElement>(".peek");
  const peekTop = peek ? peek.scrollTop : 0;
  syncChildren(root, ordered);
  if (peek && peek.scrollTop !== peekTop) peek.scrollTop = peekTop;
  if (active instanceof HTMLElement && active !== document.activeElement && active.isConnected) active.focus();

  if (before.size > 0) {
    seen.forEach((key) => {
      const el = existing.get(key);
      const prev = before.get(key);
      if (!el || !prev) return;
      const next = el.getBoundingClientRect();
      const dy = prev.top - next.top;
      if (dy !== 0 && Number.isFinite(dy) && typeof el.animate === "function") {
        el.animate([{ transform: `translateY(${dy}px)` }, { transform: "translateY(0)" }], { duration: 200, easing: "ease-out" });
      }
    });
  }
}
