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
  root.replaceChildren();
  const brand = document.createElement("span");
  brand.className = "brand";
  brand.textContent = "herdr";
  root.appendChild(brand);
  const chips = document.createElement("div");
  chips.className = "chips";
  for (const server of list.servers) {
    const chip = document.createElement("span");
    chip.className = `chip ${server.connection.state}`;
    chip.innerHTML = `<i></i>`;
    chip.appendChild(document.createTextNode(server.label));
    chip.title =
      server.connection.state === "offline"
        ? `${server.connection.reason} · ${retryText(server.connection.retry_at, nowMs)}${server.transport ? ` · ${server.transport}` : ""}`
        : server.transport || server.connection.state;
    chips.appendChild(chip);
  }
  root.appendChild(chips);
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
  el.addEventListener("animationend", () => el.classList.remove("flash", "enter"));
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

function buildGroup(group: OfflineGroup, nowMs: number): HTMLElement {
  const el = document.createElement("div");
  el.className = "group";
  el.dataset.server = group.server_id;
  el.innerHTML = `<div class="group-line"><span class="group-label"></span><span class="group-status"></span></div>`;
  el.querySelector(".group-label")!.textContent = group.label;
  el.querySelector(".group-status")!.textContent = group.retry_at === null ? group.reason : `${group.reason} · ${retryText(group.retry_at, nowMs)}`;
  return el;
}

function emptyText(list: AgentList): string | null {
  if (list.rows.length > 0 || list.offline.some((g) => g.rows.length > 0)) return null;
  const anyOnline = list.servers.some((s) => s.connection.state === "online");
  if (anyOnline) return "No agents running";
  const local = list.servers.find((s) => s.id === "local");
  if (local && local.connection.state === "offline") return "herdr is not running";
  return list.servers.length === 0 ? "No servers configured" : "Connecting…";
}

/**
 * Re-render the list. Existing row elements are reused (keyed), the drawer is
 * moved after the open row, offline groups follow the online rows, and rows
 * that moved get a short FLIP transition.
 */
export function renderList(root: HTMLElement, list: AgentList, nowMs: number, opts: RenderOptions): void {
  const existing = new Map<string, RowElement>();
  root.querySelectorAll<RowElement>(".row[data-key]").forEach((el) => existing.set(el.dataset.key!, el));
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
    ordered.push(buildGroup(group, nowMs));
    for (const row of group.rows) place(row, true);
  }
  const empty = emptyText(list);
  if (empty) {
    const el = document.createElement("div");
    el.className = "empty";
    el.textContent = empty;
    ordered.push(el);
  }
  if (opts.openKey !== null && !seen.has(opts.openKey) && opts.drawer.isConnected) opts.drawer.remove();
  root.replaceChildren(...ordered);

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
