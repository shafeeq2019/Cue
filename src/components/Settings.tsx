import { useEffect, useState } from "react";
import { api, asCueError, type AppInfo, type DiagEntry, type HotkeyCheck, type Mode, type ModelRef, type Theme } from "../api";
import { displayAccelerator, eventToAccelerator } from "../keys";
import { ConfirmButton } from "./ConfirmButton";
import { MODE_LABELS } from "./ModelPicker";
import { Providers } from "./Providers";

export type Tab = "providers" | "models" | "general" | "privacy" | "diagnostics" | "about";

const TABS: { id: Tab; title: string }[] = [
  { id: "providers", title: "AI Providers" },
  { id: "models", title: "Models & Modes" },
  { id: "general", title: "General" },
  { id: "privacy", title: "Privacy & Data" },
  { id: "diagnostics", title: "Diagnostics" },
  { id: "about", title: "About" },
];

const refKey = (r: ModelRef | null) => (r ? `${r.connection_id}::${r.model_id}` : "");
const parseRef = (v: string): ModelRef | null => {
  if (!v) return null;
  const [connection_id, ...rest] = v.split("::");
  return { connection_id, model_id: rest.join("::") };
};

function ModelSelect({ info, value, onChange, emptyLabel }: { info: AppInfo; value: ModelRef | null; onChange: (r: ModelRef | null) => void; emptyLabel: string }) {
  return (
    <select value={refKey(value)} onChange={(e) => onChange(parseRef(e.target.value))}>
      <option value="">{emptyLabel}</option>
      {info.settings.connections
        .filter((c) => c.enabled && c.models.length)
        .map((c) => (
          <optgroup key={c.id} label={c.label}>
            {c.models.map((m) => (
              <option key={m.id} value={refKey({ connection_id: c.id, model_id: m.id })}>
                {m.display_name}
              </option>
            ))}
          </optgroup>
        ))}
    </select>
  );
}

function Models({ info, onChanged, toast }: { info: AppInfo; onChanged: () => void; toast: (m: string) => void }) {
  const save = (f: () => Promise<void>) =>
    f()
      .then(onChanged)
      .catch((e) => toast(asCueError(e).message));
  const hasModels = info.settings.connections.some((c) => c.enabled && c.models.length);
  return (
    <div className="stack">
      <h3>Default model</h3>
      {!hasModels && <p className="muted">Connect a provider and load its models first.</p>}
      <ModelSelect info={info} value={info.settings.default_model} emptyLabel="First available model" onChange={(r) => save(() => api.setDefaultModel(r))} />
      <h3>Modes</h3>
      <p className="muted small">
        Modes only ever use models from your connected, working accounts. “Automatic” suggests a model by family name (e.g. Haiku/Flash/mini for Fast); pick one explicitly to override.
      </p>
      {(["fast", "reasoning", "writing"] as Mode[]).map((m) => {
        const explicit = info.settings.modes[m];
        const resolved = info.modes[m];
        return (
          <div className="mode-row" key={m}>
            <div>
              <div>{MODE_LABELS[m].title}</div>
              <div className="muted small">{MODE_LABELS[m].desc}</div>
            </div>
            <div className="stack-tight">
              <ModelSelect info={info} value={explicit} emptyLabel="Automatic" onChange={(r) => save(() => api.setModeModel(m, r))} />
              <span className="muted small">
                {resolved ? `Uses: ${resolved.model_id}` : "Not available — no suitable model in your accounts"}
              </span>
            </div>
          </div>
        );
      })}
    </div>
  );
}

function HotkeyRecorder({ info, onChanged }: { info: AppInfo; onChanged: () => void }) {
  const [recording, setRecording] = useState(false);
  const [candidate, setCandidate] = useState<string | null>(null);
  const [check, setCheck] = useState<HotkeyCheck | null>(null);
  const [result, setResult] = useState<string | null>(null);

  useEffect(() => {
    if (!candidate) return;
    api.checkHotkey(candidate).then(setCheck);
  }, [candidate]);

  return (
    <div className="stack-tight">
      <div className="row">
        <button
          className={`hotkey ${recording ? "recording" : ""}`}
          onClick={() => {
            setRecording(true);
            setCandidate(null);
            setCheck(null);
            setResult(null);
          }}
          onKeyDown={(e) => {
            if (!recording) return;
            e.preventDefault();
            e.stopPropagation();
            if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.metaKey) {
              setRecording(false);
              return;
            }
            const acc = eventToAccelerator(e.nativeEvent);
            if (acc) {
              setCandidate(acc);
              setRecording(false);
            }
          }}
          onBlur={() => setRecording(false)}
        >
          {recording ? "Press the new shortcut…" : displayAccelerator(candidate ?? info.settings.hotkey, info.platform)}
        </button>
        {candidate && check?.ok && (
          <button
            className="primary small"
            onClick={async () => {
              const r = await api.setHotkey(candidate);
              if (r.ok) {
                setResult("Shortcut saved.");
                setCandidate(null);
                onChanged();
              } else {
                setResult(r.error);
              }
            }}
          >
            Use this shortcut
          </button>
        )}
        {candidate && (
          <button className="ghost small" onClick={() => (setCandidate(null), setCheck(null))}>
            Cancel
          </button>
        )}
      </div>
      {check?.error && <div className="err-text small">{check.error}</div>}
      {check?.warnings.map((w) => (
        <div key={w} className="warn-text small">
          ⚠ {w}
        </div>
      ))}
      {result && <div className="small">{result}</div>}
      <span className="muted small">Default: {displayAccelerator(info.default_hotkey, info.platform)}. Conflicts with system shortcuts and other apps are detected when saving.</span>
    </div>
  );
}

function General({ info, onChanged, toast }: { info: AppInfo; onChanged: () => void; toast: (m: string) => void }) {
  const [launch, setLaunch] = useState<boolean | null>(null);
  const [sys, setSys] = useState(info.settings.system_prompt ?? "");
  useEffect(() => {
    api.getLaunchAtLogin().then(setLaunch).catch(() => setLaunch(null));
  }, []);
  const prefs = (p: Parameters<typeof api.updatePrefs>[0]) =>
    api
      .updatePrefs(p)
      .then(onChanged)
      .catch((e) => toast(asCueError(e).message));
  return (
    <div className="stack">
      <h3>Global shortcut</h3>
      <HotkeyRecorder info={info} onChanged={onChanged} />
      <h3>Appearance</h3>
      <div className="segmented">
        {(["system", "light", "dark"] as Theme[]).map((t) => (
          <button key={t} className={info.settings.theme === t ? "on" : ""} onClick={() => prefs({ theme: t })}>
            {t[0].toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>
      <h3>Behaviour</h3>
      <label className="toggle">
        <input type="checkbox" checked={info.settings.hide_on_blur} onChange={(e) => prefs({ hide_on_blur: e.target.checked })} />
        Hide Cue when it loses focus
      </label>
      {launch !== null && (
        <label className="toggle">
          <input
            type="checkbox"
            checked={launch}
            onChange={(e) =>
              api
                .setLaunchAtLogin(e.target.checked)
                .then(setLaunch)
                .catch((err) => toast(asCueError(err).message))
            }
          />
          Launch Cue at login
        </label>
      )}
      <h3>Custom instructions</h3>
      <textarea className="sys" rows={3} value={sys} onChange={(e) => setSys(e.target.value)} placeholder="Optional system prompt sent with every request, e.g. “Answer concisely.”" />
      <div className="row">
        <button className="small" onClick={() => prefs({ system_prompt: sys }).then(() => toast("Saved"))}>
          Save instructions
        </button>
      </div>
    </div>
  );
}

function Privacy({ info, onChanged, toast }: { info: AppInfo; onChanged: () => void; toast: (m: string) => void }) {
  return (
    <div className="stack">
      <h3>What leaves your computer</h3>
      <ul className="notes">
        <li>Only when you send a message: the message, the earlier turns of the same conversation and your custom instructions go to the provider of the selected model — nothing else.</li>
        <li>Every answer shows which connection, provider and model produced it, and how it is billed.</li>
        <li>The first time you use a connection, Cue asks before sending anything to it.</li>
        <li>No telemetry or analytics. Cue only contacts the providers you configure.</li>
      </ul>
      <h3>Chat history</h3>
      <label className="toggle">
        <input
          type="checkbox"
          checked={info.settings.persist_history}
          onChange={(e) =>
            api
              .updatePrefs({ persist_history: e.target.checked })
              .then(onChanged)
              .catch((err) => toast(asCueError(err).message))
          }
        />
        Keep chat history on this device
      </label>
      <p className="muted small">
        Off by default: conversations live in memory and disappear when Cue quits. When on, they are stored unencrypted in Cue's app-data folder with owner-only file permissions.
      </p>
      <div className="row">
        <ConfirmButton
          label="Delete all chat history"
          confirmLabel="Delete everything? Click again"
          onConfirm={async () => {
            await api.clearHistory();
            toast("History cleared");
          }}
        />
      </div>
      <h3>Credentials</h3>
      <p className="muted small">
        API keys and sign-in tokens are stored only in the {info.platform === "macos" ? "macOS Keychain" : info.platform === "windows" ? "Windows Credential Manager" : "Secret Service keyring"}. They are never written to
        Cue's settings file, never shown again after entry and never included in logs. Cue never asks for your provider passwords — sign-ins happen in your browser or in the provider's own app.
      </p>
      {info.secret_store_error && <div className="notice warn">{info.secret_store_error}</div>}
    </div>
  );
}

function Diagnostics({ toast }: { toast: (m: string) => void }) {
  const [entries, setEntries] = useState<DiagEntry[]>([]);
  const load = () => api.diagnostics().then(setEntries);
  useEffect(() => {
    load();
  }, []);
  const text = entries.map((e) => `${new Date(e.at * 1000).toISOString()} [${e.level}] ${e.message}`).join("\n");
  return (
    <div className="stack">
      <div className="section-head">
        <h3>Diagnostics</h3>
        <div className="row">
          <button className="small ghost" onClick={load}>
            Refresh
          </button>
          <button className="small ghost" onClick={() => api.copyText(text).then(() => toast("Log copied"))} disabled={!entries.length}>
            Copy
          </button>
          <button className="small ghost" onClick={() => api.clearDiagnostics().then(load)}>
            Clear
          </button>
        </div>
      </div>
      <p className="muted small">Recent events of this session (in memory only). Credentials are redacted; message contents are never logged.</p>
      <pre className="log">{text || "No events."}</pre>
    </div>
  );
}

function About({ info }: { info: AppInfo }) {
  return (
    <div className="stack">
      <h3>Cue {info.version}</h3>
      <p className="muted">A fast, keyboard-first assistant for many AI providers.</p>
      <p className="small">
        Platform: {info.platform} · Shortcut: {displayAccelerator(info.settings.hotkey, info.platform)}
      </p>
      <p className="small muted">
        Cue is not affiliated with Anthropic, OpenAI or Google. Provider names are used only to identify the services you connect.
      </p>
    </div>
  );
}

export function SettingsView({ info, initialTab, onBack, onChanged, toast }: { info: AppInfo; initialTab: Tab; onBack: () => void; onChanged: () => void; toast: (m: string) => void }) {
  const [tab, setTab] = useState<Tab>(initialTab);
  useEffect(() => setTab(initialTab), [initialTab]);
  return (
    <div className="page">
      <header className="bar" data-tauri-drag-region>
        <button className="ghost small" onClick={onBack}>
          ← Chat
        </button>
        <div className="bar-title" data-tauri-drag-region>
          Settings
        </div>
        <span />
      </header>
      <div className="settings">
        <nav className="tabs">
          {TABS.map((t) => (
            <button key={t.id} className={tab === t.id ? "on" : ""} onClick={() => setTab(t.id)}>
              {t.title}
            </button>
          ))}
        </nav>
        <div className="page-body">
          {tab === "providers" && <Providers info={info} onChanged={onChanged} toast={toast} />}
          {tab === "models" && <Models info={info} onChanged={onChanged} toast={toast} />}
          {tab === "general" && <General info={info} onChanged={onChanged} toast={toast} />}
          {tab === "privacy" && <Privacy info={info} onChanged={onChanged} toast={toast} />}
          {tab === "diagnostics" && <Diagnostics toast={toast} />}
          {tab === "about" && <About info={info} />}
        </div>
      </div>
    </div>
  );
}
