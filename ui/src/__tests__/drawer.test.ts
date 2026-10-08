import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Drawer, KEYPAD, type Api } from "../drawer";
import type { AgentRow } from "../types";

const row = (over: Partial<AgentRow> = {}): AgentRow => ({
  server_id: "local",
  server_label: "local",
  pane_id: "w1:p1",
  workspace_label: "ws",
  agent_kind: "claude",
  name: "reviewer",
  status: "working",
  since: null,
  flash_seq: 0,
  ...over,
});

function fakeApi(): Api & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    peek: vi.fn(async (server, pane) => {
      calls.push(`peek ${server} ${pane}`);
      return { text: "line 1\nline 2", truncated: false };
    }),
    sendKey: vi.fn(async (server, pane, key) => {
      calls.push(`key ${server} ${pane} ${key}`);
    }),
    sendMessage: vi.fn(async (server, pane, text) => {
      calls.push(`msg ${server} ${pane} ${text}`);
    }),
  };
}

const flush = () => Promise.resolve().then(() => Promise.resolve());

let api: ReturnType<typeof fakeApi>;
let drawer: Drawer;

beforeEach(() => {
  vi.useFakeTimers();
  document.body.innerHTML = "";
  api = fakeApi();
  drawer = new Drawer(api, 2000, 5000);
  document.body.appendChild(drawer.el);
});

afterEach(() => {
  drawer.close();
  vi.useRealTimers();
});

describe("Drawer", () => {
  it("opens on peek, polls every 2s, stops on close", async () => {
    drawer.open(row());
    await flush();
    expect(drawer.currentKey).toBe("local|w1:p1");
    expect(drawer.el.querySelector(".tab.on")!.textContent).toBe("Peek");
    expect(drawer.el.querySelector(".peek")!.textContent).toBe("line 1\nline 2");
    expect(api.calls.filter((c) => c.startsWith("peek")).length).toBe(1);
    await vi.advanceTimersByTimeAsync(4100);
    expect(api.calls.filter((c) => c.startsWith("peek")).length).toBe(3);
    drawer.close();
    await vi.advanceTimersByTimeAsync(4000);
    expect(api.calls.filter((c) => c.startsWith("peek")).length).toBe(3);
    expect(drawer.currentKey).toBeNull();
  });

  it("shows the keypad under the peek only for blocked rows, and always on the keys tab", async () => {
    drawer.open(row({ status: "working" }));
    await flush();
    expect(drawer.el.querySelector(".keys")).toBeNull();
    drawer.update(row({ status: "blocked" }));
    await flush();
    expect(drawer.el.querySelectorAll(".keys .key").length).toBe(KEYPAD.length);
    (drawer.el.querySelector('[data-tab="keys"]') as HTMLElement).click();
    expect(drawer.el.querySelectorAll(".keys .key").length).toBe(10);
    (drawer.el.querySelector('.key[data-key="1"]') as HTMLElement).click();
    await flush();
    expect(api.calls).toContain("key local w1:p1 1");
    expect(drawer.el.querySelector('.key[data-key="enter"]')!.className).toContain("primary");
    expect(drawer.el.querySelector('.key[data-key="ctrl+c"]')!.className).toContain("caution");
  });

  it("sends a message on Enter, clears on success, keeps text and shows an error on failure", async () => {
    drawer.open(row());
    (drawer.el.querySelector('[data-tab="message"]') as HTMLElement).click();
    const input = drawer.el.querySelector(".send input") as HTMLInputElement;
    input.value = "continue";
    input.form!.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(api.calls).toContain("msg local w1:p1 continue");
    expect(input.value).toBe("");

    (api.sendMessage as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: "agent_blocked", message: "Waiting at a prompt. Answer it with the keypad first." });
    input.value = "again";
    input.form!.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(input.value).toBe("again");
    const error = drawer.el.querySelector(".error") as HTMLElement;
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("Waiting at a prompt. Answer it with the keypad first.");
    await vi.advanceTimersByTimeAsync(5100);
    expect(error.hidden).toBe(true);
  });

  it("toggle closes the same row and switches to another", async () => {
    drawer.toggle(row({ pane_id: "a" }));
    expect(drawer.currentKey).toBe("local|a");
    drawer.toggle(row({ pane_id: "b" }));
    expect(drawer.currentKey).toBe("local|b");
    drawer.toggle(row({ pane_id: "b" }));
    expect(drawer.currentKey).toBeNull();
  });

  it("peek failure shows the error but keeps polling", async () => {
    (api.peek as ReturnType<typeof vi.fn>).mockRejectedValueOnce({ code: "transport", message: "Could not reach local." });
    drawer.open(row());
    await flush();
    expect((drawer.el.querySelector(".error") as HTMLElement).textContent).toBe("Could not reach local.");
    await vi.advanceTimersByTimeAsync(2100);
    expect(drawer.el.querySelector(".peek")!.textContent).toBe("line 1\nline 2");
  });
});
