import { Drawer } from "./drawer";
import { renderHeader, renderList } from "./render";
import { api } from "./tauri";
import { EMPTY_LIST, rowKey, type AgentList, type AgentRow } from "./types";

const header = document.querySelector<HTMLElement>("#header")!;
const listEl = document.querySelector<HTMLElement>("#list")!;
const banner = document.querySelector<HTMLElement>("#banner")!;

let list: AgentList = EMPTY_LIST;
const drawer = new Drawer(api);

function findRow(key: string): AgentRow | null {
  return list.rows.find((r) => rowKey(r) === key) ?? list.offline.flatMap((g) => g.rows).find((r) => rowKey(r) === key) ?? null;
}

function render(): void {
  const now = Date.now();
  renderHeader(header, list, now);
  const openKey = drawer.currentKey;
  if (openKey !== null) {
    const current = findRow(openKey);
    if (current) drawer.update(current);
    else drawer.close();
  }
  renderList(listEl, list, now, {
    openKey: drawer.currentKey,
    drawer: drawer.el,
    onRowClick: (row) => {
      drawer.toggle(row);
      render();
    },
  });
}

let receivedEvent = false;

function setBanner(message: string | null): void {
  banner.hidden = message === null;
  banner.textContent = message ?? "";
}

async function start(): Promise<void> {
  await api.onAgents((next) => {
    receivedEvent = true;
    list = next;
    render();
  });
  await api.onConfigError(setBanner);
  const [initial, configError] = await Promise.all([api.getAgents(), api.getConfigError()]);
  if (!receivedEvent) {
    list = initial;
    render();
  }
  setBanner(configError);
}

void start();
setInterval(render, 15_000);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && drawer.isOpen()) {
    drawer.close();
    render();
  }
});
