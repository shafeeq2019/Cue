// User-facing wording for providers, auth methods and billing. Kept in one
// place so the UI never over-promises what a connection can do.
import type { Attribution, AuthMethod, Billing, Connection, ProviderKind, Usage } from "./api";

export interface ProviderOption {
  kind: ProviderKind;
  auth: AuthMethod;
  title: string;
  subtitle: string;
  /** Short badge describing how it connects. */
  badge: string;
  keyPlaceholder?: string;
  keyUrl?: string;
  needsBaseUrl?: boolean;
  optionalKey?: boolean;
  notes: string[];
}

export const PROVIDER_OPTIONS: ProviderOption[] = [
  {
    kind: "anthropic",
    auth: "api_key",
    title: "Anthropic Claude",
    subtitle: "Claude API key from platform.claude.com",
    badge: "API key",
    keyPlaceholder: "sk-ant-…",
    keyUrl: "https://platform.claude.com/settings/keys",
    notes: [
      "Billed per token to your Anthropic API account (Claude Console).",
      "Claude Pro/Max subscriptions do not include API usage. Anthropic does not allow third-party apps to sign in with Claude.ai accounts.",
    ],
  },
  {
    kind: "claude_code",
    auth: "local_cli",
    title: "Claude Code (local)",
    subtitle: "Use your installed Claude Code — e.g. signed in with Claude Pro/Max",
    badge: "Claude Code",
    notes: [
      "Cue runs your locally installed, unmodified Claude Code CLI with all tools disabled. You sign in only inside Claude Code (`claude auth login`), using Anthropic's own flow — Cue never sees your Claude credentials.",
      "Usage counts toward whatever Claude Code is signed in with (your Claude plan limits, or API billing if it uses an API key).",
      "Anthropic's terms say subscription use is intended for ordinary, individual use of Claude Code. Use this for your own personal use only. Requires Claude Code to be installed.",
    ],
  },
  {
    kind: "openai",
    auth: "chatgpt_oauth",
    title: "ChatGPT plan",
    subtitle: "Official “Sign in with ChatGPT” — uses your ChatGPT Plus/Pro plan",
    badge: "ChatGPT sign-in",
    notes: [
      "Opens chatgpt.com in your browser. You sign in there; Cue never sees your password.",
      "Requests count toward your ChatGPT plan's usage limits (Plus limits are shared with other apps). Manage access at chatgpt.com/settings/usage.",
      "Preview feature from OpenAI for open-source and local apps; availability depends on your plan and workspace.",
    ],
  },
  {
    kind: "openai",
    auth: "api_key",
    title: "OpenAI API",
    subtitle: "API key from platform.openai.com",
    badge: "API key",
    keyPlaceholder: "sk-…",
    keyUrl: "https://platform.openai.com/api-keys",
    notes: ["Billed per token to your OpenAI API account. Not covered by ChatGPT subscriptions."],
  },
  {
    kind: "gemini",
    auth: "api_key",
    title: "Google Gemini",
    subtitle: "API key from Google AI Studio",
    badge: "API key",
    keyPlaceholder: "AIza…",
    keyUrl: "https://aistudio.google.com/apikey",
    notes: [
      "Gemini API usage is separate from Google AI Pro/Ultra subscriptions (free tier or Cloud Billing, per Google's terms).",
    ],
  },
  {
    kind: "openai_compatible",
    auth: "none",
    title: "Local model server",
    subtitle: "Ollama, LM Studio or any OpenAI-compatible server",
    badge: "Local / custom",
    needsBaseUrl: true,
    optionalKey: true,
    notes: ["Runs wherever the server runs. For Ollama use http://localhost:11434/v1."],
  },
  {
    kind: "openai_compatible",
    auth: "api_key",
    title: "Other provider (OpenAI-compatible)",
    subtitle: "OpenRouter, Groq, Mistral, … via their official API",
    badge: "API key",
    needsBaseUrl: true,
    keyPlaceholder: "API key",
    notes: ["Billed per token by that service according to its own pricing."],
  },
];

export function providerName(kind: ProviderKind): string {
  return {
    anthropic: "Anthropic",
    openai: "OpenAI",
    gemini: "Google Gemini",
    openai_compatible: "OpenAI-compatible",
    claude_code: "Claude Code",
  }[kind];
}

/** How the connection authenticates, in plain words. */
export function authLabel(c: Pick<Connection, "auth" | "kind" | "account">): string {
  switch (c.auth) {
    case "api_key":
      return "API key";
    case "chatgpt_oauth":
      return "ChatGPT plan (official sign-in)";
    case "local_cli":
      return claudeCodeAuth(c.account?.auth_detail ?? null, c.account?.plan ?? null);
    case "none":
      return "No authentication";
  }
}

function claudeCodeAuth(detail: string | null, plan: string | null): string {
  switch (detail) {
    case "claude.ai":
      return plan ? `Claude subscription (${plan}) via Claude Code` : "Claude subscription via Claude Code";
    case "api_key":
    case "api_key_helper":
      return "Anthropic API key via Claude Code";
    case "oauth_token":
      return "Claude Code token";
    case "third_party":
      return "Cloud provider via Claude Code";
    case "none":
      return "Claude Code (not signed in)";
    default:
      return "Claude Code sign-in";
  }
}

export function billingLabel(b: Billing, authDetail?: string | null): string {
  switch (b) {
    case "api_usage":
      return "Paid API usage";
    case "subscription_plan":
      return "ChatGPT plan";
    case "external_tool":
      if (authDetail === "claude.ai") return "Claude plan via Claude Code";
      if (authDetail === "api_key" || authDetail === "api_key_helper") return "API billing via Claude Code";
      return "Via Claude Code";
    case "self_managed":
      return "Local / self-managed";
  }
}

export function billingExplanation(b: Billing): string {
  switch (b) {
    case "api_usage":
      return "Billed per token to your API account. Not covered by chat subscriptions.";
    case "subscription_plan":
      return "Counts toward your ChatGPT plan's usage limits. No API charges.";
    case "external_tool":
      return "Uses Claude Code's own sign-in. Counts toward that account's plan limits or API billing.";
    case "self_managed":
      return "Handled by the server you configured.";
  }
}

export function attributionLine(a: Attribution): string {
  const model = a.served_model && a.served_model !== a.model ? `${a.model} → ${a.served_model}` : a.model;
  return `${a.connection_label} · ${model}`;
}

export function formatTokens(u: Usage | null): string | null {
  if (!u || (u.input_tokens == null && u.output_tokens == null)) return null;
  const n = (x: number | null) => (x == null ? "?" : x.toLocaleString());
  let s = `${n(u.input_tokens)} in · ${n(u.output_tokens)} out tokens`;
  if (u.reported_cost_usd != null && u.reported_cost_usd > 0) s += ` · $${u.reported_cost_usd.toFixed(5)} (reported by provider)`;
  return s;
}

export function usageUrl(c: Connection): string | null {
  if (c.auth === "chatgpt_oauth") return "https://chatgpt.com/settings/usage";
  switch (c.kind) {
    case "anthropic":
      return "https://platform.claude.com/usage";
    case "openai":
      return "https://platform.openai.com/usage";
    case "gemini":
      return "https://aistudio.google.com/usage";
    default:
      return null;
  }
}

/** Mirrors `AuthMethod::billing` in cue-core. */
export function connectionBilling(c: Pick<Connection, "auth">): Billing {
  if (c.auth === "chatgpt_oauth") return "subscription_plan";
  if (c.auth === "local_cli") return "external_tool";
  if (c.auth === "none") return "self_managed";
  return "api_usage";
}
