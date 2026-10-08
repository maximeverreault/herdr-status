export type AgentStatus = "blocked" | "working" | "done" | "idle" | "unknown";
export type ServerKind = "local" | "wsl" | "remote";

export type ConnectionState =
  | { state: "connecting" }
  | { state: "online" }
  | { state: "offline"; reason: string; retry_at: number };

export interface AgentRow {
  server_id: string;
  server_label: string;
  pane_id: string;
  workspace_label: string;
  agent_kind: string | null;
  name: string;
  status: AgentStatus;
  since: number | null;
  flash_seq: number;
}

export interface ServerChip {
  id: string;
  label: string;
  kind: ServerKind;
  connection: ConnectionState;
  transport: string;
}

export interface OfflineGroup {
  server_id: string;
  label: string;
  reason: string;
  retry_at: number | null;
  rows: AgentRow[];
}

export interface Counts {
  blocked: number;
  working: number;
  done: number;
  idle: number;
  unknown: number;
}

export interface AgentList {
  servers: ServerChip[];
  rows: AgentRow[];
  offline: OfflineGroup[];
  counts: Counts;
}

export interface PeekResult {
  text: string;
  truncated: boolean;
}

export interface ActionError {
  code: string;
  message: string;
}

export const EMPTY_LIST: AgentList = {
  servers: [],
  rows: [],
  offline: [],
  counts: { blocked: 0, working: 0, done: 0, idle: 0, unknown: 0 },
};

export function rowKey(row: AgentRow): string {
  return `${row.server_id}|${row.pane_id}`;
}
