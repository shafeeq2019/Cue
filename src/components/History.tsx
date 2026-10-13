import { useEffect, useState } from "react";
import { api, asCueError, type AppInfo, type ConversationSummary } from "../api";
import { ConfirmButton } from "./ConfirmButton";

export function History({
  info,
  onOpen,
  onBack,
  toast,
}: {
  info: AppInfo;
  onOpen: (id: string) => void;
  onBack: () => void;
  toast: (m: string) => void;
}) {
  const [items, setItems] = useState<ConversationSummary[]>([]);
  const [q, setQ] = useState("");
  const load = () => api.listConversations().then(setItems).catch((e) => toast(asCueError(e).message));
  useEffect(() => {
    load();
  }, []);

  const shown = items.filter((i) => i.title.toLowerCase().includes(q.trim().toLowerCase()));
  return (
    <div className="page">
      <header className="bar" data-tauri-drag-region>
        <button className="ghost small" onClick={onBack}>
          ← Chat
        </button>
        <div className="bar-title" data-tauri-drag-region>
          History
        </div>
        <ConfirmButton
          className="ghost small danger"
          disabled={!items.length}
          label="Clear all"
          confirmLabel="Delete all?"
          onConfirm={async () => {
            await api.clearHistory();
            toast("History cleared");
            load();
          }}
        />
      </header>
      <div className="page-body">
        <p className="muted small">
          {info.settings.persist_history
            ? "History is saved on this device (Settings → Privacy & Data)."
            : "History is kept in memory only and is gone when Cue quits. You can change this under Privacy & Data."}
        </p>
        <input className="search" placeholder="Search conversations…" value={q} onChange={(e) => setQ(e.target.value)} autoFocus />
        {shown.length === 0 && <div className="muted empty-small">No conversations.</div>}
        <ul className="history-list">
          {shown.map((c) => (
            <li key={c.id}>
              <button className="history-item" onClick={() => onOpen(c.id)}>
                <span className="history-title">{c.title}</span>
                <span className="muted small">
                  {new Date(c.updated_at * 1000).toLocaleString()} · {c.message_count} messages
                </span>
              </button>
              <button
                className="ghost tiny"
                aria-label="Delete conversation"
                onClick={async () => {
                  await api.deleteConversation(c.id);
                  load();
                }}
              >
                Delete
              </button>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
