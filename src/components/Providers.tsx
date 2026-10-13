import { useState } from "react";
import { api, asCueError, type AppInfo, type ClaudeCodeStatus, type Connection, type CueError } from "../api";
import { authLabel, billingExplanation, billingLabel, connectionBilling, PROVIDER_OPTIONS, providerName, usageUrl, type ProviderOption } from "../labels";
import { ConfirmButton } from "./ConfirmButton";
import { ErrorCard } from "./ErrorCard";

function StatusDot({ c }: { c: Connection }) {
  const s = c.status?.state ?? "untested";
  const label = !c.enabled ? "Disabled" : s === "ok" ? (c.status?.error ? "Connected (warning)" : "Connected") : s === "error" ? "Error" : "Not tested";
  return (
    <span className="status">
      <span className={`dot ${!c.enabled ? "" : s === "ok" ? "ok" : s === "error" ? "err" : ""}`} />
      {label}
    </span>
  );
}

function ConnectionCard({ c, info, onChanged, toast }: { c: Connection; info: AppInfo; onChanged: () => void; toast: (m: string) => void }) {
  const [busy, setBusy] = useState<string | null>(null);
  const [newKey, setNewKey] = useState("");
  const [editing, setEditing] = useState(false);
  const [label, setLabel] = useState(c.label);
  const [baseUrl, setBaseUrl] = useState(c.base_url ?? "");
  const [cliPath, setCliPath] = useState(c.cli_path ?? "");
  const [showModels, setShowModels] = useState(false);
  const usage = info.session_usage[c.id];
  const billing = connectionBilling(c);

  const run = async (name: string, f: () => Promise<unknown>, ok?: string) => {
    setBusy(name);
    try {
      await f();
      if (ok) toast(ok);
    } catch (e) {
      toast(asCueError(e).message);
    } finally {
      setBusy(null);
      onChanged();
    }
  };

  return (
    <div className={`card conn ${c.enabled ? "" : "off"}`}>
      <div className="conn-head">
        <div className="avatar">{c.label.slice(0, 1).toUpperCase()}</div>
        <div className="conn-title">
          <div className="conn-name">{c.label}</div>
          <div className="muted small">
            {providerName(c.kind)} · {authLabel(c)}
            {c.account?.email ? ` · ${c.account.email}` : ""}
          </div>
        </div>
        <StatusDot c={c} />
      </div>

      <div className="conn-badges">
        <span className={`badge billing-${billing}`} title={billingExplanation(billing)}>
          {billingLabel(billing, c.account?.auth_detail)}
        </span>
        <span className="badge neutral">{c.models.length} models</span>
        {c.base_url && <span className="badge neutral mono">{c.base_url}</span>}
        {c.has_secret && c.auth === "api_key" && <span className="badge neutral" title="Stored in the operating system's credential store">🔒 Key in system keychain</span>}
      </div>
      <p className="muted small">{billingExplanation(billing)}</p>

      {c.status?.error && <ErrorCard error={c.status.error} />}

      {usage && (
        <div className="muted small">
          This session: {usage.requests} requests · {usage.input_tokens.toLocaleString()} in / {usage.output_tokens.toLocaleString()} out tokens
          {usage.reported_cost_usd > 0 ? ` · $${usage.reported_cost_usd.toFixed(4)} reported by provider` : ""}
          {billing === "api_usage" ? " · costs per token are shown in your provider console" : ""}
        </div>
      )}

      <div className="row wrap">
        <button className="small" disabled={!!busy} onClick={() => run("test", () => api.testConnection(c.id), "Connection OK")}>
          {busy === "test" ? "Testing…" : "Test & load models"}
        </button>
        {c.auth === "chatgpt_oauth" && (
          <button className="small" disabled={!!busy} onClick={() => run("reauth", () => api.chatgptSignIn(c.id, null), "Signed in")}>
            {busy === "reauth" ? "Waiting for browser…" : "Sign in again"}
          </button>
        )}
        <button className="small ghost" onClick={() => setEditing((e) => !e)}>
          {editing ? "Close" : "Edit"}
        </button>
        <button className="small ghost" onClick={() => setShowModels((s) => !s)} disabled={!c.models.length}>
          {showModels ? "Hide models" : "Show models"}
        </button>
        {usageUrl(c) && (
          <button className="small ghost" onClick={() => api.openExternal(usageUrl(c)!).catch((e) => toast(asCueError(e).message))}>
            Usage ↗
          </button>
        )}
        <label className="toggle small">
          <input type="checkbox" checked={c.enabled} onChange={(e) => run("enable", () => api.updateConnection({ id: c.id, enabled: e.target.checked }))} />
          Enabled
        </label>
        <ConfirmButton
          className="small ghost danger"
          disabled={!!busy}
          label="Remove"
          confirmLabel="Remove connection and its stored credentials?"
          onConfirm={() =>
            run("remove", async () => {
              const r = await api.removeConnection(c.id);
              if (!r.remote_revocation_confirmed) toast("Removed locally. Remote sign-out could not be confirmed — you can disconnect Cue in your ChatGPT settings.");
            })
          }
        />
      </div>

      {editing && (
        <div className="edit">
          <label>
            Name
            <input value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
          {c.kind === "openai_compatible" && (
            <label>
              Base URL
              <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="http://localhost:11434/v1" />
            </label>
          )}
          {c.kind === "claude_code" && (
            <label>
              Claude Code path (optional)
              <input value={cliPath} onChange={(e) => setCliPath(e.target.value)} placeholder="Auto-detect" />
            </label>
          )}
          {c.auth === "api_key" && (
            <label>
              Replace API key
              <input type="password" autoComplete="off" value={newKey} onChange={(e) => setNewKey(e.target.value)} placeholder="Leave empty to keep the stored key" />
            </label>
          )}
          <div className="row">
            <button
              className="primary small"
              onClick={() =>
                run(
                  "save",
                  async () => {
                    await api.updateConnection({
                      id: c.id,
                      label,
                      base_url: c.kind === "openai_compatible" ? baseUrl : undefined,
                      cli_path: c.kind === "claude_code" ? cliPath : undefined,
                      api_key: newKey || undefined,
                    });
                    setNewKey("");
                    setEditing(false);
                    await api.testConnection(c.id);
                  },
                  "Saved",
                )
              }
            >
              Save
            </button>
          </div>
        </div>
      )}

      {showModels && (
        <ul className="model-list">
          {c.models.map((m) => (
            <li key={m.id}>
              <span>{m.display_name}</span>
              <span className="muted mono small">{m.id}</span>
              {m.context_window && <span className="muted small">{Math.round(m.context_window / 1000)}k ctx</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function AddProvider({ onDone, onCancel, toast }: { onDone: () => void; onCancel: () => void; toast: (m: string) => void }) {
  const [opt, setOpt] = useState<ProviderOption | null>(null);
  const [label, setLabel] = useState("");
  const [key, setKey] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [cliPath, setCliPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<CueError | null>(null);
  const [cc, setCc] = useState<ClaudeCodeStatus | null>(null);

  if (!opt) {
    return (
      <div className="add">
        <div className="section-head">
          <h3>Add a provider</h3>
          <button className="ghost small" onClick={onCancel}>
            Cancel
          </button>
        </div>
        <div className="options">
          {PROVIDER_OPTIONS.map((o) => (
            <button
              key={`${o.kind}-${o.auth}`}
              className="option"
              onClick={() => {
                setOpt(o);
                setBaseUrl(o.auth === "none" ? "http://localhost:11434/v1" : "");
              }}
            >
              <span className="option-title">
                {o.title} <span className="badge neutral">{o.badge}</span>
              </span>
              <span className="muted small">{o.subtitle}</span>
            </button>
          ))}
        </div>
      </div>
    );
  }

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      let c: Connection;
      if (opt.auth === "chatgpt_oauth") {
        c = await api.chatgptSignIn(null, label || null);
      } else {
        c = await api.addConnection({
          kind: opt.kind,
          auth: opt.optionalKey && key.trim() ? "api_key" : opt.auth,
          label: label || opt.title,
          base_url: baseUrl || null,
          cli_path: cliPath || null,
          api_key: key || null,
        });
      }
      setKey("");
      if (c.status?.state === "error" && c.status.error) {
        toast("Added, but the connection test failed — see details on the card.");
      } else {
        toast(`Connected · ${c.models.length} models available`);
      }
      onDone();
    } catch (e) {
      setError(asCueError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="add">
      <div className="section-head">
        <h3>{opt.title}</h3>
        <button className="ghost small" onClick={() => (busy ? api.chatgptCancelSignIn() : setOpt(null))}>
          {busy && opt.auth === "chatgpt_oauth" ? "Cancel sign-in" : "Back"}
        </button>
      </div>
      <ul className="notes">
        {opt.notes.map((n, i) => (
          <li key={i}>{n}</li>
        ))}
      </ul>
      <label>
        Name
        <input value={label} onChange={(e) => setLabel(e.target.value)} placeholder={opt.title} />
      </label>
      {opt.needsBaseUrl && (
        <label>
          Base URL
          <input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://…/v1" />
        </label>
      )}
      {(opt.auth === "api_key" || opt.optionalKey) && (
        <label>
          <span>
            API key {opt.optionalKey && <span className="muted">(optional)</span>}
          </span>
          <input type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)} placeholder={opt.keyPlaceholder} />
          <span className="muted small">
            Stored only in your system keychain. Never shown again or sent anywhere except {providerName(opt.kind)}.
            {opt.keyUrl && (
              <>
                {" "}
                <button className="link" onClick={() => api.openExternal(opt.keyUrl!)}>
                  Get a key ↗
                </button>
              </>
            )}
          </span>
        </label>
      )}
      {opt.kind === "claude_code" && (
        <>
          <label>
            <span>
              Claude Code path <span className="muted">(optional)</span>
            </span>
            <input value={cliPath} onChange={(e) => setCliPath(e.target.value)} placeholder="Auto-detect" />
          </label>
          <div className="row">
            <button
              className="small"
              onClick={async () => {
                setError(null);
                try {
                  setCc(await api.claudeCodeStatus(cliPath || null));
                } catch (e) {
                  setCc(null);
                  setError(asCueError(e));
                }
              }}
            >
              Check Claude Code
            </button>
            {cc && (
              <span className="small">
                {cc.version ?? "Claude Code"} · {cc.logged_in ? <span className="ok-text">signed in ({authLabel({ auth: "local_cli", kind: "claude_code", account: { auth_detail: cc.auth_method, plan: cc.subscription, email: null, subject: null } })})</span> : <span className="err-text">not signed in — run `claude` in a terminal and use /login</span>}
              </span>
            )}
          </div>
        </>
      )}
      {error && <ErrorCard error={error} />}
      <div className="row">
        {opt.auth === "chatgpt_oauth" ? (
          <button className="primary" disabled={busy} onClick={submit}>
            {busy ? "Waiting for your browser…" : "Continue with ChatGPT"}
          </button>
        ) : (
          <button className="primary" disabled={busy || (opt.auth === "api_key" && !opt.optionalKey && !key.trim())} onClick={submit}>
            {busy ? "Connecting…" : "Connect"}
          </button>
        )}
      </div>
    </div>
  );
}

export function Providers({ info, onChanged, toast }: { info: AppInfo; onChanged: () => void; toast: (m: string) => void }) {
  const [adding, setAdding] = useState(info.settings.connections.length === 0);
  return (
    <div className="providers">
      {info.secret_store_error && <div className="notice warn">{info.secret_store_error}</div>}
      {adding ? (
        <AddProvider
          toast={toast}
          onCancel={() => setAdding(false)}
          onDone={() => {
            setAdding(false);
            onChanged();
          }}
        />
      ) : (
        <>
          <div className="section-head">
            <h3>AI Providers</h3>
            <button className="primary small" onClick={() => setAdding(true)}>
              + Add provider
            </button>
          </div>
          {info.settings.connections.length === 0 && <p className="muted">No connections yet.</p>}
          {info.settings.connections.map((c) => (
            <ConnectionCard key={c.id} c={c} info={info} onChanged={onChanged} toast={toast} />
          ))}
          <SubscriptionFacts />
        </>
      )}
    </div>
  );
}

function SubscriptionFacts() {
  return (
    <details className="facts">
      <summary>Which subscriptions can Cue use?</summary>
      <ul>
        <li>
          <strong>ChatGPT Plus/Pro:</strong> yes, through OpenAI's official “Sign in with ChatGPT” (preview for open-source/local apps). Counts toward your plan limits.
        </li>
        <li>
          <strong>Claude Pro/Max:</strong> not directly — Anthropic does not allow third-party apps to offer Claude.ai sign-in. Cue can use your locally installed Claude Code, which you sign in to yourself; or use a Claude API key (billed separately).
        </li>
        <li>
          <strong>Google AI Pro/Ultra:</strong> no — Gemini API usage is billed separately (or via the free tier) with an AI Studio key.
        </li>
        <li>API keys are always billed per token by the provider; chat subscriptions never cover API usage.</li>
      </ul>
    </details>
  );
}
