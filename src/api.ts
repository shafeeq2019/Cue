// Typed bridge to the Rust backend. The UI never receives credentials:
// API keys are only ever *sent* (write-only) and stored in the OS keychain.
import { Channel, invoke } from "@tauri-apps/api/core";

export type ProviderKind = "anthropic" | "openai" | "gemini" | "openai_compatible" | "claude_code";
export type AuthMethod = "api_key" | "chatgpt_oauth" | "local_cli" | "none";
export type Billing = "api_usage" | "subscription_plan" | "external_tool" | "self_managed";
export type Theme = "system" | "light" | "dark";
export type Mode = "fast" | "reasoning" | "writing";
export type Role = "user" | "assistant";
export type StopReason = "complete" | "max_tokens" | "refusal" | "other";

export type ErrorKind =
  | "auth"
  | "permission_denied"
  | "model_not_found"
  | "rate_limited"
  | "quota_exceeded"
  | "subscription_limit"
  | "overloaded"
  | "invalid_request"
  | "context_too_long"
  | "refusal"
  | "network"
  | "timeout"
  | "cancelled"
  | "not_configured"
  | "secret_store"
  | "unknown";

export interface CueError {
  kind: ErrorKind;
  message: string;
  hint: string | null;
  detail: string | null;
  status: number | null;
  retryable: boolean;
  request_id: string | null;
}

export interface ModelInfo {
  id: string;
  display_name: string;
  context_window: number | null;
  max_output_tokens: number | null;
}

export interface AccountInfo {
  email: string | null;
  subject: string | null;
  plan: string | null;
  auth_detail: string | null;
}

export interface ConnectionStatus {
  state: "untested" | "ok" | "error";
  checked_at: number;
  error: CueError | null;
}

export interface Connection {
  id: string;
  kind: ProviderKind;
  auth: AuthMethod;
  label: string;
  base_url: string | null;
  cli_path: string | null;
  enabled: boolean;
  account: AccountInfo | null;
  oauth_client_id: string | null;
  models: ModelInfo[];
  status: ConnectionStatus | null;
  has_secret: boolean;
}

export interface ModelRef {
  connection_id: string;
  model_id: string;
}

export interface Settings {
  version: number;
  hotkey: string;
  theme: Theme;
  connections: Connection[];
  default_model: ModelRef | null;
  modes: { fast: ModelRef | null; reasoning: ModelRef | null; writing: ModelRef | null };
  persist_history: boolean;
  hide_on_blur: boolean;
  acknowledged_connections: string[];
  host_id: string;
  system_prompt: string | null;
}

export interface UsageTotals {
  requests: number;
  input_tokens: number;
  output_tokens: number;
  reported_cost_usd: number;
}

export interface AppInfo {
  version: string;
  platform: "macos" | "windows" | "linux";
  default_hotkey: string;
  hotkey_display: string;
  secret_store_error: string | null;
  hotkey_error: string | null;
  settings: Settings;
  session_usage: Record<string, UsageTotals>;
  modes: { fast: ModelRef | null; reasoning: ModelRef | null; writing: ModelRef | null; default: ModelRef | null };
}

export interface Usage {
  input_tokens: number | null;
  output_tokens: number | null;
  reported_cost_usd: number | null;
}

export interface Attribution {
  connection_id: string;
  connection_label: string;
  provider: ProviderKind;
  model: string;
  served_model: string | null;
  billing: Billing;
  auth_detail: string | null;
}

export interface StoredMessage {
  id: string;
  role: Role;
  content: string;
  created_at: number;
  attribution: Attribution | null;
  usage: Usage | null;
  stop: StopReason | null;
  error: CueError | null;
  pending: boolean;
}

export interface Conversation {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
  messages: StoredMessage[];
  last_model: ModelRef | null;
}

export interface ConversationSummary {
  id: string;
  title: string;
  updated_at: number;
  message_count: number;
}

export type ChatEvent =
  | { type: "started"; request_id: string; conversation_id: string; message_id: string; attribution: Attribution }
  | { type: "delta"; text: string }
  | { type: "model"; model: string }
  | { type: "notice"; message: string }
  | { type: "done"; stop: StopReason; usage: Usage }
  | { type: "error"; error: CueError };

export interface HotkeyCheck {
  ok: boolean;
  normalized: string | null;
  error: string | null;
  warnings: string[];
}

export interface ClaudeCodeStatus {
  executable: string;
  version: string | null;
  logged_in: boolean;
  auth_method: string | null;
  subscription: string | null;
}

export interface DiagEntry {
  at: number;
  level: string;
  message: string;
}

export interface NewConnection {
  kind: ProviderKind;
  auth: AuthMethod;
  label: string;
  base_url?: string | null;
  cli_path?: string | null;
  api_key?: string | null;
}

/** Normalise anything thrown by `invoke` into a CueError. */
export function asCueError(e: unknown): CueError {
  if (e && typeof e === "object" && "kind" in e && "message" in e) return e as CueError;
  return {
    kind: "unknown",
    message: typeof e === "string" ? e : "Unexpected error.",
    hint: null,
    detail: e instanceof Error ? e.message : null,
    status: null,
    retryable: false,
    request_id: null,
  };
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw asCueError(e);
  }
}

export const api = {
  appInfo: () => call<AppInfo>("get_app_info"),
  addConnection: (conn: NewConnection) => call<Connection>("add_connection", { conn }),
  updateConnection: (patch: { id: string; label?: string; base_url?: string; cli_path?: string; enabled?: boolean; api_key?: string }) =>
    call<Connection>("update_connection", { patch }),
  removeConnection: (id: string) => call<{ remote_revocation_confirmed: boolean }>("remove_connection", { id }),
  testConnection: (id: string) => call<Connection>("test_connection", { id }),
  claudeCodeStatus: (cliPath: string | null) => call<ClaudeCodeStatus>("claude_code_status", { cliPath }),
  chatgptSignIn: (connectionId: string | null, label: string | null) => call<Connection>("chatgpt_sign_in", { connectionId, label }),
  chatgptCancelSignIn: () => call<void>("chatgpt_cancel_sign_in"),
  setDefaultModel: (model: ModelRef | null) => call<void>("set_default_model", { model }),
  setModeModel: (mode: Mode, model: ModelRef | null) => call<void>("set_mode_model", { mode, model }),
  updatePrefs: (prefs: { theme?: Theme; hide_on_blur?: boolean; persist_history?: boolean; system_prompt?: string }) =>
    call<void>("update_prefs", { prefs }),
  checkHotkey: (accelerator: string) => call<HotkeyCheck>("check_hotkey", { accelerator }),
  setHotkey: (accelerator: string) => call<HotkeyCheck>("set_hotkey", { accelerator }),
  acknowledgeConnection: (id: string) => call<void>("acknowledge_connection", { id }),
  getLaunchAtLogin: () => call<boolean>("get_launch_at_login"),
  setLaunchAtLogin: (enabled: boolean) => call<boolean>("set_launch_at_login", { enabled }),
  sendMessage: (req: { conversation_id: string | null; model: ModelRef; text: string }, onEvent: (e: ChatEvent) => void) => {
    const channel = new Channel<ChatEvent>();
    channel.onmessage = onEvent;
    return call<{ request_id: string; conversation_id: string; user_message: StoredMessage; assistant_message: StoredMessage }>(
      "send_message",
      { req, channel },
    );
  },
  cancelMessage: (requestId: string) => call<void>("cancel_message", { requestId }),
  listConversations: () => call<ConversationSummary[]>("list_conversations"),
  getConversation: (id: string) => call<Conversation | null>("get_conversation", { id }),
  deleteConversation: (id: string) => call<void>("delete_conversation", { id }),
  clearHistory: () => call<void>("clear_history"),
  copyText: (text: string) => call<void>("copy_text", { text }),
  hideWindow: () => call<void>("hide_window"),
  openExternal: (url: string) => call<void>("open_external", { url }),
  diagnostics: () => call<DiagEntry[]>("get_diagnostics"),
  clearDiagnostics: () => call<void>("clear_diagnostics"),
};
