import { useEffect, useMemo, useRef, useState } from "react";
import type { AppInfo, Connection, Mode, ModelRef } from "../api";
import { billingLabel, connectionBilling } from "../labels";

export type Selection = { mode: Mode | "manual"; model: ModelRef | null };

export const MODE_LABELS: Record<Mode, { title: string; desc: string }> = {
  fast: { title: "Fast", desc: "Short questions, quick answers" },
  reasoning: { title: "Reasoning", desc: "Complex, multi-step problems" },
  writing: { title: "Writing", desc: "Write, rewrite, summarise" },
};

export function modelName(info: AppInfo, ref: ModelRef | null): string {
  if (!ref) return "No model";
  const c = info.settings.connections.find((c) => c.id === ref.connection_id);
  const m = c?.models.find((m) => m.id === ref.model_id);
  return m?.display_name ?? ref.model_id;
}

export function connectionOf(info: AppInfo, ref: ModelRef | null): Connection | undefined {
  return ref ? info.settings.connections.find((c) => c.id === ref.connection_id) : undefined;
}

type Item =
  | { kind: "mode"; mode: Mode; ref: ModelRef | null }
  | { kind: "model"; conn: Connection; ref: ModelRef; name: string; isDefault: boolean };

export function ModelPicker({
  info,
  current,
  onSelect,
  onClose,
  onOpenSettings,
}: {
  info: AppInfo;
  current: Selection;
  onSelect: (s: Selection) => void;
  onClose: () => void;
  onOpenSettings: () => void;
}) {
  const [q, setQ] = useState("");
  const [idx, setIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => inputRef.current?.focus(), []);

  const items = useMemo<Item[]>(() => {
    const needle = q.trim().toLowerCase();
    const modes: Item[] = (["fast", "reasoning", "writing"] as Mode[])
      .filter((m) => !needle || MODE_LABELS[m].title.toLowerCase().includes(needle))
      .map((m) => ({ kind: "mode", mode: m, ref: info.modes[m] }));
    const models: Item[] = [];
    for (const c of info.settings.connections) {
      if (!c.enabled) continue;
      for (const m of c.models) {
        const hay = `${m.display_name} ${m.id} ${c.label}`.toLowerCase();
        if (needle && !hay.includes(needle)) continue;
        const ref = { connection_id: c.id, model_id: m.id };
        const d = info.modes.default;
        models.push({ kind: "model", conn: c, ref, name: m.display_name, isDefault: !!d && d.connection_id === c.id && d.model_id === m.id });
      }
    }
    return [...modes, ...models];
  }, [q, info]);

  useEffect(() => setIdx(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-idx="${idx}"]`)?.scrollIntoView({ block: "nearest" });
  }, [idx]);

  const choose = (it: Item | undefined) => {
    if (!it) return;
    if (it.kind === "mode") {
      if (!it.ref) return;
      onSelect({ mode: it.mode, model: it.ref });
    } else {
      onSelect({ mode: "manual", model: it.ref });
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setIdx((i) => Math.min(i + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(items[idx]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  };

  let lastConn = "";
  const noModels = !info.settings.connections.some((c) => c.enabled && c.models.length > 0);

  return (
    <div className="picker-backdrop" onMouseDown={onClose}>
      <div className="picker" onMouseDown={(e) => e.stopPropagation()} onKeyDown={onKey}>
        <input ref={inputRef} className="picker-input" placeholder="Search models…" value={q} onChange={(e) => setQ(e.target.value)} />
        <div className="picker-list" ref={listRef}>
          {noModels && (
            <div className="picker-empty">
              No models available yet.{" "}
              <button className="link" onClick={onOpenSettings}>
                Connect a provider
              </button>
            </div>
          )}
          {items.map((it, i) => {
            const active = i === idx;
            if (it.kind === "mode") {
              const selected = current.mode === it.mode;
              return (
                <div key={`mode-${it.mode}`}>
                  {i === 0 && <div className="picker-group">Modes</div>}
                  <div
                    data-idx={i}
                    className={`picker-item ${active ? "active" : ""} ${!it.ref ? "disabled" : ""}`}
                    onMouseEnter={() => setIdx(i)}
                    onClick={() => choose(it)}
                    title={it.ref ? undefined : "No suitable model in your connected accounts. Set one in Settings → Models."}
                  >
                    <span className="picker-main">
                      {MODE_LABELS[it.mode].title}
                      <span className="muted"> — {MODE_LABELS[it.mode].desc}</span>
                    </span>
                    <span className="picker-side">{it.ref ? modelName(info, it.ref) : "Not available"}</span>
                    {selected && <span className="check">✓</span>}
                  </div>
                </div>
              );
            }
            const header = it.conn.id !== lastConn;
            lastConn = it.conn.id;
            const selected = current.mode === "manual" && current.model?.connection_id === it.ref.connection_id && current.model?.model_id === it.ref.model_id;
            return (
              <div key={`${it.conn.id}/${it.ref.model_id}`}>
                {header && (
                  <div className="picker-group">
                    {it.conn.label} <span className="muted">· {billingLabel(connectionBilling(it.conn), it.conn.account?.auth_detail)}</span>
                  </div>
                )}
                <div data-idx={i} className={`picker-item ${active ? "active" : ""}`} onMouseEnter={() => setIdx(i)} onClick={() => choose(it)}>
                  <span className="picker-main">{it.name}</span>
                  <span className="picker-side mono">{it.name !== it.ref.model_id ? it.ref.model_id : ""}</span>
                  {it.isDefault && <span className="tag">default</span>}
                  {selected && <span className="check">✓</span>}
                </div>
              </div>
            );
          })}
        </div>
        <div className="picker-foot">
          <span>↑↓ navigate · ↵ select · esc close</span>
          <button className="link" onClick={onOpenSettings}>
            Manage providers
          </button>
        </div>
      </div>
    </div>
  );
}
