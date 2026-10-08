import { beforeEach, describe, expect, it, vi } from "vitest";
import { start } from "../main";
import type { AppApi } from "../tauri";
import { EMPTY_LIST, type AgentList } from "../types";

const flush = async () => {
  for (let i = 0; i < 6; i++) await Promise.resolve();
};

const withRow = (name: string): AgentList => ({
  ...EMPTY_LIST,
  servers: [{ id: "local", label: "local", kind: "local", connection: { state: "online" }, transport: "x" }],
  rows: [{ server_id: "local", server_label: "local", pane_id: "p", workspace_label: "ws", agent_kind: null, name, status: "working", since: null, flash_seq: 0 }],
});

interface Fake extends AppApi {
  agentsCb: (l: AgentList) => void;
  configCb: (m: string | null) => void;
}

function fake(over: Partial<AppApi> = {}): Fake {
  const f = {
    agentsCb: () => {},
    configCb: () => {},
    getAgents: async () => EMPTY_LIST,
    getConfigError: async () => null,
    peek: async () => ({ text: "", truncated: false }),
    sendKey: async () => {},
    sendMessage: async () => {},
    onAgents: async (cb: (l: AgentList) => void) => {
      f.agentsCb = cb;
      return () => {};
    },
    onConfigError: async (cb: (m: string | null) => void) => {
      f.configCb = cb;
      return () => {};
    },
    ...over,
  } as Fake;
  return f;
}

let els: { header: HTMLElement; list: HTMLElement; banner: HTMLElement };

beforeEach(() => {
  document.body.innerHTML = `<header id="h"></header><main id="l"></main><div id="banner" hidden></div>`;
  els = { header: document.getElementById("h")!, list: document.getElementById("l")!, banner: document.getElementById("banner")! };
});

describe("start", () => {
  it("renders Connecting… immediately", async () => {
    const api = fake({ getAgents: () => new Promise(() => {}) });
    void start(api, els);
    expect(els.list.querySelector(".empty")!.textContent).toBe("Connecting…");
  });

  it("ignores the snapshot when an agents event arrived first", async () => {
    let resolve!: (l: AgentList) => void;
    const api = fake({ getAgents: () => new Promise<AgentList>((r) => (resolve = r)) });
    const done = start(api, els);
    await flush();
    api.agentsCb(withRow("fresh"));
    resolve(withRow("old"));
    await done;
    expect(els.list.querySelector(".name")!.textContent).toBe("fresh");
  });

  it("does not overwrite a config event with the fetched value", async () => {
    let resolve!: (m: string | null) => void;
    const api = fake({ getConfigError: () => new Promise<string | null>((r) => (resolve = r)) });
    const done = start(api, els);
    await flush();
    api.configCb("bad config");
    resolve(null);
    await done;
    expect(els.banner.hidden).toBe(false);
    expect(els.banner.textContent).toBe("bad config");
  });

  it("shows the fetched config error", async () => {
    await start(fake({ getConfigError: async () => "oops" }), els);
    expect(els.banner.textContent).toBe("oops");
  });

  it("a rejected getAgents still renders and shows the banner", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    await start(fake({ getAgents: async () => Promise.reject({ code: "x", message: "boom" }) }), els);
    expect(els.header.querySelector(".brand")).not.toBeNull();
    expect(els.banner.hidden).toBe(false);
    expect(els.banner.textContent).toBe("Could not load agents: boom");
  });

  it("a_rejected_getAgents_banner_clears_on_the_next_agents_event", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const api = fake({ getAgents: async () => Promise.reject({ message: "boom" }) });
    await start(api, els);
    expect(els.banner.hidden).toBe(false);
    api.agentsCb(withRow("x"));
    expect(els.banner.hidden).toBe(true);
  });

  it("a_fetched_config_error_is_not_replaced_by_a_getAgents_failure", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    await start(fake({ getConfigError: async () => "bad config", getAgents: async () => Promise.reject({ message: "boom" }) }), els);
    expect(els.banner.textContent).toBe("bad config");
  });
});
