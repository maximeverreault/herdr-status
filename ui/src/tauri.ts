import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Api } from "./drawer";
import type { AgentList, PeekResult } from "./types";

export interface AppApi extends Api {
  getAgents(): Promise<AgentList>;
  getConfigError(): Promise<string | null>;
  onAgents(cb: (list: AgentList) => void): Promise<UnlistenFn>;
  onConfigError(cb: (message: string | null) => void): Promise<UnlistenFn>;
}

export const api: AppApi = {
  getAgents: () => invoke<AgentList>("get_agents"),
  getConfigError: () => invoke<string | null>("get_config_error"),
  peek: (server, pane) => invoke<PeekResult>("peek", { server, pane }),
  sendKey: (server, pane, key) => invoke<void>("send_key", { server, pane, key }),
  sendMessage: (server, pane, text) => invoke<void>("send_message", { server, pane, text }),
  onAgents: (cb) => listen<AgentList>("agents", (e) => cb(e.payload)),
  onConfigError: (cb) => listen<string | null>("config_error", (e) => cb(e.payload)),
};
