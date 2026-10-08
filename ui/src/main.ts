import { Drawer } from "./drawer";
import { renderHeader, renderList } from "./render";
import { api, type AppApi } from "./tauri";
import { EMPTY_LIST, rowKey, type AgentList, type AgentRow } from "./types";

export interface Elements {
  header: HTMLElement;
  list: HTMLElement;
  banner: HTMLElement;
}

export async function start(appApi: AppApi, els: Elements): Promise<void> {
  let list: AgentList = EMPTY_LIST;
  const drawer = new Drawer(appApi);

  const findRow = (key: string): AgentRow | null =>
    list.rows.find((r) => rowKey(r) === key) ?? list.offline.flatMap((g) => g.rows).find((r) => rowKey(r) === key) ?? null;

  const render = (): void => {
    const now = Date.now();
    renderHeader(els.header, list, now);
    const openKey = drawer.currentKey;
    if (openKey !== null) {
      const current = findRow(openKey);
      if (current) drawer.update(current);
      else drawer.close();
    }
    renderList(els.list, list, now, {
      openKey: drawer.currentKey,
      drawer: drawer.el,
      onRowClick: (row) => {
        drawer.toggle(row);
        render();
      },
    });
  };

  let configMsg: string | null = null;
  let loadError: string | null = null;
  const showBanner = (): void => {
    const message = configMsg ?? loadError;
    els.banner.hidden = message === null;
    els.banner.textContent = message ?? "";
  };

  setInterval(render, 15_000);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && drawer.isOpen()) {
      drawer.close();
      render();
    }
  });

  render();

  let receivedAgentsEvent = false;
  let receivedConfigEvent = false;
  try {
    await appApi.onAgents((next) => {
      receivedAgentsEvent = true;
      loadError = null;
      showBanner();
      list = next;
      render();
    });
    await appApi.onConfigError((message) => {
      receivedConfigEvent = true;
      configMsg = message;
      showBanner();
    });
    const [agents, configError] = await Promise.allSettled([appApi.getAgents(), appApi.getConfigError()]);
    if (configError.status === "fulfilled" && !receivedConfigEvent) {
      configMsg = configError.value;
      showBanner();
    }
    if (agents.status === "fulfilled") {
      if (!receivedAgentsEvent) {
        list = agents.value;
        render();
      }
    } else {
      throw agents.reason;
    }
  } catch (err) {
    console.error("could not load agents", err);
    {
      const message = err && typeof err === "object" && typeof (err as { message?: unknown }).message === "string" ? (err as { message: string }).message : String(err);
      loadError = `Could not load agents: ${message}`;
      showBanner();
    }
  }
}

const header = document.querySelector<HTMLElement>("#header");
const listEl = document.querySelector<HTMLElement>("#list");
const banner = document.querySelector<HTMLElement>("#banner");
if (header && listEl && banner) void start(api, { header, list: listEl, banner });
