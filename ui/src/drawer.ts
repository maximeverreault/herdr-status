import { rowKey, type ActionError, type AgentRow, type PeekResult } from "./types";

export interface Api {
  peek(server: string, pane: string): Promise<PeekResult>;
  sendKey(server: string, pane: string, key: string): Promise<void>;
  sendMessage(server: string, pane: string, text: string): Promise<void>;
}

export type Tab = "peek" | "keys" | "message";

/** Same order and allow-list as `herdr_status_core::actions::KEYPAD`. */
export const KEYPAD: ReadonlyArray<{ key: string; label: string; cls: string }> = [
  { key: "1", label: "1", cls: "primary" },
  { key: "2", label: "2", cls: "" },
  { key: "3", label: "3", cls: "" },
  { key: "y", label: "y", cls: "" },
  { key: "n", label: "n", cls: "" },
  { key: "up", label: "↑", cls: "" },
  { key: "down", label: "↓", cls: "" },
  { key: "enter", label: "Enter", cls: "primary" },
  { key: "esc", label: "Esc", cls: "caution" },
  { key: "ctrl+c", label: "^C", cls: "caution" },
];

function messageOf(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) return String((err as ActionError).message);
  return String(err);
}

export class Drawer {
  readonly el: HTMLElement;
  private row: AgentRow | null = null;
  private tab: Tab = "peek";
  private pollTimer: ReturnType<typeof setInterval> | null = null;
  private errorTimer: ReturnType<typeof setTimeout> | null = null;
  private peekText = "";
  private peekTruncated = false;
  private draft = "";

  constructor(
    private readonly api: Api,
    private readonly peekIntervalMs = 2000,
    private readonly errorMs = 5000,
  ) {
    this.el = document.createElement("div");
    this.el.className = "drawer";
    this.el.addEventListener("click", (e) => e.stopPropagation());
  }

  get currentKey(): string | null {
    return this.row ? rowKey(this.row) : null;
  }

  isOpen(): boolean {
    return this.row !== null;
  }

  toggle(row: AgentRow): void {
    if (this.currentKey === rowKey(row)) this.close();
    else this.open(row);
  }

  open(row: AgentRow): void {
    this.stopPolling();
    this.row = row;
    this.tab = "peek";
    this.peekText = "";
    this.peekTruncated = false;
    this.render();
    this.startPolling();
  }

  close(): void {
    this.stopPolling();
    this.row = null;
    this.el.replaceChildren();
    this.el.remove();
  }

  /** Called on every list render while open, so status changes update the keypad. */
  update(row: AgentRow): void {
    if (!this.row || rowKey(row) !== rowKey(this.row)) return;
    const statusChanged = row.status !== this.row.status;
    this.row = row;
    if (statusChanged) this.render();
  }

  private startPolling(): void {
    void this.refreshPeek();
    this.pollTimer = setInterval(() => {
      if (this.tab === "peek") void this.refreshPeek();
    }, this.peekIntervalMs);
  }

  private stopPolling(): void {
    if (this.pollTimer !== null) clearInterval(this.pollTimer);
    this.pollTimer = null;
  }

  private async refreshPeek(): Promise<void> {
    if (!this.row) return;
    try {
      const result = await this.api.peek(this.row.server_id, this.row.pane_id);
      this.peekText = result.text;
      this.peekTruncated = result.truncated;
      this.applyPeek();
    } catch (err) {
      this.showError(err);
    }
  }

  private applyPeek(): void {
    const pre = this.el.querySelector<HTMLElement>(".peek");
    if (!pre) return;
    pre.textContent = this.peekText;
    pre.scrollTop = pre.scrollHeight;
    const note = this.el.querySelector<HTMLElement>(".peek-note");
    if (note) note.hidden = !this.peekTruncated;
  }

  private async press(key: string): Promise<void> {
    if (!this.row) return;
    try {
      await this.api.sendKey(this.row.server_id, this.row.pane_id, key);
    } catch (err) {
      this.showError(err);
    }
  }

  private async send(input: HTMLInputElement): Promise<void> {
    if (!this.row) return;
    const text = input.value;
    try {
      await this.api.sendMessage(this.row.server_id, this.row.pane_id, text);
      input.value = "";
      this.draft = "";
    } catch (err) {
      this.draft = text;
      this.showError(err);
    }
  }

  private showError(err: unknown): void {
    const box = this.el.querySelector<HTMLElement>(".error");
    if (!box) return;
    box.textContent = messageOf(err);
    box.hidden = false;
    if (this.errorTimer !== null) clearTimeout(this.errorTimer);
    this.errorTimer = setTimeout(() => {
      box.hidden = true;
    }, this.errorMs);
  }

  private keypad(): HTMLElement {
    const keys = document.createElement("div");
    keys.className = "keys";
    for (const { key, label, cls } of KEYPAD) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `key ${cls}`.trim();
      b.dataset.key = key;
      b.textContent = label;
      b.addEventListener("click", () => void this.press(key));
      keys.appendChild(b);
    }
    return keys;
  }

  private render(): void {
    if (!this.row) return;
    this.el.replaceChildren();
    const tabs = document.createElement("div");
    tabs.className = "tabs";
    for (const [id, label] of [["peek", "Peek"], ["keys", "Keys"], ["message", "Message"]] as const) {
      const b = document.createElement("button");
      b.type = "button";
      b.className = `tab${this.tab === id ? " on" : ""}`;
      b.dataset.tab = id;
      b.textContent = label;
      b.addEventListener("click", () => {
        this.tab = id;
        this.render();
        if (id === "peek") void this.refreshPeek();
      });
      tabs.appendChild(b);
    }
    this.el.appendChild(tabs);

    const content = document.createElement("div");
    content.className = "content";
    if (this.tab === "peek") {
      const pre = document.createElement("pre");
      pre.className = "peek";
      content.appendChild(pre);
      const note = document.createElement("div");
      note.className = "peek-note";
      note.textContent = "output truncated";
      note.hidden = true;
      content.appendChild(note);
      if (this.row.status === "blocked") content.appendChild(this.keypad());
    } else if (this.tab === "keys") {
      content.appendChild(this.keypad());
    } else {
      const form = document.createElement("form");
      form.className = "send";
      const input = document.createElement("input");
      input.type = "text";
      input.placeholder = "Message to agent…";
      input.value = this.draft;
      const button = document.createElement("button");
      button.type = "submit";
      button.textContent = "Send";
      form.append(input, button);
      form.addEventListener("submit", (e) => {
        e.preventDefault();
        void this.send(input);
      });
      content.appendChild(form);
      queueMicrotask(() => input.focus());
    }
    this.el.appendChild(content);

    const error = document.createElement("div");
    error.className = "error";
    error.hidden = true;
    this.el.appendChild(error);
    this.applyPeek();
  }
}
