import { forwardRef, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";
import { api, asCueError, type AppInfo, type ChatEvent, type CueError, type StoredMessage } from "../api";
import { attributionLine, billingExplanation, billingLabel, connectionBilling, formatTokens, providerName } from "../labels";
import { displayAccelerator, isMod, modLabel } from "../keys";
import { ErrorCard } from "./ErrorCard";
import { Markdown } from "./Markdown";
import { connectionOf, MODE_LABELS, ModelPicker, modelName, type Selection } from "./ModelPicker";

export interface ChatHandle {
  focus: () => void;
  newChat: () => void;
  openConversation: (id: string) => Promise<void>;
}

interface Props {
  info: AppInfo;
  onOpenSettings: (tab?: string) => void;
  onOpenHistory: () => void;
  onInfoChanged: () => void;
  toast: (msg: string) => void;
}

let localSeq = 0;
const localId = () => `local-${++localSeq}`;

export const Chat = forwardRef<ChatHandle, Props>(function Chat({ info, onOpenSettings, onOpenHistory, onInfoChanged, toast }, ref) {
  const [conversationId, setConversationId] = useState<string | null>(null);
  const [messages, setMessages] = useState<StoredMessage[]>([]);
  const [input, setInput] = useState("");
  const [selection, setSelection] = useState<Selection>(() => ({ mode: "manual", model: info.modes.default }));
  const [pickerOpen, setPickerOpen] = useState(false);
  const [streaming, setStreaming] = useState<{ requestId: string | null; localId: string } | null>(null);
  const [pendingAck, setPendingAck] = useState<string | null>(null);
  const [sendError, setSendError] = useState<CueError | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const platform = info.platform;

  // Keep the selection valid when connections/models change.
  useEffect(() => {
    setSelection((s) => {
      if (s.mode !== "manual") return { mode: s.mode, model: info.modes[s.mode] };
      const stillThere = s.model && info.settings.connections.some((c) => c.enabled && c.id === s.model!.connection_id && c.models.some((m) => m.id === s.model!.model_id));
      return stillThere ? s : { mode: "manual", model: info.modes.default };
    });
  }, [info]);

  const focus = useCallback(() => requestAnimationFrame(() => inputRef.current?.focus()), []);

  const newChat = useCallback(() => {
    if (streaming) return;
    setConversationId(null);
    setMessages([]);
    setSendError(null);
    setPendingAck(null);
    focus();
  }, [streaming, focus]);

  useImperativeHandle(ref, () => ({
    focus,
    newChat,
    openConversation: async (id: string) => {
      const c = await api.getConversation(id);
      if (!c) return;
      setConversationId(c.id);
      setMessages(c.messages);
      if (c.last_model && info.settings.connections.some((x) => x.id === c.last_model!.connection_id)) {
        setSelection({ mode: "manual", model: c.last_model });
      }
      stickToBottom.current = true;
      focus();
    },
  }));

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages]);

  // Auto-grow the input.
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 160)}px`;
  }, [input]);

  const conn = connectionOf(info, selection.model);
  const acknowledged = conn ? info.settings.acknowledged_connections.includes(conn.id) : false;

  const updateLocal = (id: string, f: (m: StoredMessage) => StoredMessage) => setMessages((ms) => ms.map((m) => (m.id === id ? f(m) : m)));

  const send = async (text: string, skipAck = false) => {
    const trimmed = text.trim();
    if (!trimmed || streaming) return;
    setSendError(null);
    if (!selection.model || !conn) {
      setSendError({
        kind: "not_configured",
        message: info.settings.connections.length ? "No model selected." : "No AI provider connected yet.",
        hint: "Connect a provider and load its models under AI Providers.",
        detail: null,
        status: null,
        retryable: false,
        request_id: null,
      });
      return;
    }
    if (!acknowledged && !skipAck) {
      setPendingAck(trimmed);
      return;
    }
    setPendingAck(null);

    const userId = localId();
    const asstId = localId();
    const now = Math.floor(Date.now() / 1000);
    setMessages((ms) => [
      ...ms,
      { id: userId, role: "user", content: trimmed, created_at: now, attribution: null, usage: null, stop: null, error: null, pending: false },
      { id: asstId, role: "assistant", content: "", created_at: now, attribution: null, usage: null, stop: null, error: null, pending: true },
    ]);
    setInput("");
    focus();
    stickToBottom.current = true;
    setStreaming({ requestId: null, localId: asstId });

    // Batch deltas per animation frame for smooth rendering.
    let buffer = "";
    let raf = 0;
    const flush = () => {
      raf = 0;
      if (!buffer) return;
      const chunk = buffer;
      buffer = "";
      updateLocal(asstId, (m) => ({ ...m, content: m.content + chunk }));
    };
    const onEvent = (e: ChatEvent) => {
      switch (e.type) {
        case "started":
          setStreaming({ requestId: e.request_id, localId: asstId });
          setConversationId(e.conversation_id);
          updateLocal(asstId, (m) => ({ ...m, attribution: e.attribution }));
          break;
        case "delta":
          buffer += e.text;
          if (!raf) raf = requestAnimationFrame(flush);
          break;
        case "model":
          updateLocal(asstId, (m) => (m.attribution ? { ...m, attribution: { ...m.attribution, served_model: e.model } } : m));
          break;
        case "notice":
          toast(e.message);
          break;
        case "done":
          flush();
          updateLocal(asstId, (m) => ({ ...m, pending: false, usage: e.usage, stop: e.stop }));
          setStreaming(null);
          onInfoChanged();
          break;
        case "error":
          flush();
          updateLocal(asstId, (m) => ({ ...m, pending: false, error: e.error }));
          setStreaming(null);
          if (["auth", "permission_denied", "not_configured"].includes(e.error.kind)) onInfoChanged();
          break;
      }
    };
    try {
      const res = await api.sendMessage({ conversation_id: conversationId, model: selection.model, text: trimmed }, onEvent);
      setConversationId(res.conversation_id);
    } catch (err) {
      const e = asCueError(err);
      setMessages((ms) => ms.filter((m) => m.id !== asstId));
      setStreaming(null);
      setSendError(e);
    }
  };

  const stop = () => {
    if (streaming?.requestId) api.cancelMessage(streaming.requestId);
  };

  const lastAnswer = [...messages].reverse().find((m) => m.role === "assistant" && !m.pending && !m.error && m.content);

  const copy = async (text: string, label = "Copied to clipboard") => {
    try {
      await api.copyText(text);
      toast(label);
    } catch (e) {
      toast(asCueError(e).message);
    }
  };

  const pickMode = (mode: Selection["mode"]) => {
    if (mode === "manual") {
      setPickerOpen(true);
      return;
    }
    const ref = info.modes[mode];
    if (!ref) {
      toast(`${MODE_LABELS[mode].title}: no suitable model in your connected accounts`);
      return;
    }
    setSelection({ mode, model: ref });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const mod = isMod(e, platform);
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      send(input);
      return;
    }
    if (mod && e.key.toLowerCase() === "k") {
      e.preventDefault();
      setPickerOpen(true);
    } else if (mod && ["1", "2", "3", "4"].includes(e.key)) {
      e.preventDefault();
      pickMode((["fast", "reasoning", "writing", "manual"] as const)[Number(e.key) - 1]);
    } else if (mod && e.key.toLowerCase() === "n") {
      e.preventDefault();
      newChat();
    } else if (mod && e.key === ",") {
      e.preventDefault();
      onOpenSettings();
    } else if (mod && e.key.toLowerCase() === "y") {
      e.preventDefault();
      onOpenHistory();
    } else if (mod && e.shiftKey && e.key.toLowerCase() === "c") {
      e.preventDefault();
      if (lastAnswer) copy(lastAnswer.content, "Answer copied");
    } else if (mod && e.key === ".") {
      e.preventDefault();
      stop();
    }
  };

  const retry = (assistantId: string) => {
    const i = messages.findIndex((m) => m.id === assistantId);
    const q = [...messages.slice(0, i)].reverse().find((m) => m.role === "user");
    if (q) send(q.content, true);
  };

  const noProviders = info.settings.connections.length === 0;
  const m = modLabel(platform);

  return (
    <div className="chat">
      <header className="bar" data-tauri-drag-region>
        <div className="modes" role="tablist" aria-label="Mode">
          {(["fast", "reasoning", "writing"] as const).map((mode, i) => (
            <button
              key={mode}
              role="tab"
              aria-selected={selection.mode === mode}
              className={`chip ${selection.mode === mode ? "on" : ""} ${info.modes[mode] ? "" : "unavailable"}`}
              title={info.modes[mode] ? `${MODE_LABELS[mode].desc} — ${modelName(info, info.modes[mode])} (${m}${i + 1})` : "No suitable model available"}
              onClick={() => pickMode(mode)}
            >
              {MODE_LABELS[mode].title}
            </button>
          ))}
          <button className={`chip model-chip ${selection.mode === "manual" ? "on" : ""}`} onClick={() => setPickerOpen(true)} title={`Choose model (${m}K)`}>
            <span className="model-name">{selection.model ? modelName(info, selection.model) : "Choose model"}</span>
            <span className="caret">▾</span>
          </button>
        </div>
        <div className="bar-actions">
          <button className="icon" title={`New chat (${m}N)`} onClick={newChat} aria-label="New chat">
            ＋
          </button>
          <button className="icon" title={`History (${m}Y)`} onClick={onOpenHistory} aria-label="History">
            ◷
          </button>
          <button className="icon" title={`AI Providers & Settings (${m},)`} onClick={() => onOpenSettings()} aria-label="Settings">
            ⚙
          </button>
        </div>
      </header>

      <div
        className="messages"
        ref={scrollRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {messages.length === 0 && (
          <div className="empty">
            {noProviders ? (
              <>
                <div className="empty-title">Connect your first AI provider</div>
                <p className="muted">Use an API key, your ChatGPT plan, Claude Code with your Claude plan, or a local model.</p>
                <button className="primary" onClick={() => onOpenSettings("providers")}>
                  Open AI Providers
                </button>
              </>
            ) : (
              <>
                <div className="empty-title">Ask anything</div>
                <div className="shortcuts">
                  <span><kbd>↵</kbd> send</span>
                  <span><kbd>⇧↵</kbd> new line</span>
                  <span><kbd>{m}K</kbd> model</span>
                  <span><kbd>{m}1–3</kbd> modes</span>
                  <span><kbd>{platform === "macos" ? "⌘⇧C" : "Ctrl+Shift+C"}</kbd> copy answer</span>
                  <span><kbd>esc</kbd> close</span>
                </div>
                <p className="muted small">Open Cue anywhere with <kbd>{displayAccelerator(info.settings.hotkey, platform)}</kbd></p>
              </>
            )}
          </div>
        )}
        {messages.map((msg) =>
          msg.role === "user" ? (
            <div key={msg.id} className="msg user">
              <div className="bubble">{msg.content}</div>
            </div>
          ) : (
            <div key={msg.id} className="msg assistant">
              {msg.content ? <Markdown text={msg.content} onCopy={(t) => copy(t, "Code copied")} /> : msg.pending ? <div className="typing"><span /><span /><span /></div> : null}
              {msg.error && <ErrorCard error={msg.error} onRetry={() => retry(msg.id)} onOpenProviders={() => onOpenSettings("providers")} />}
              {msg.stop === "max_tokens" && <div className="notice subtle">Answer cut off at the output limit.</div>}
              {msg.stop === "refusal" && <div className="notice subtle">The model declined to answer.</div>}
              {msg.attribution && (
                <div className="meta">
                  <span className="meta-provider" title={`${providerName(msg.attribution.provider)} · ${billingExplanation(msg.attribution.billing)}`}>
                    {attributionLine(msg.attribution)}
                  </span>
                  <span className={`badge billing-${msg.attribution.billing}`}>{billingLabel(msg.attribution.billing, msg.attribution.auth_detail)}</span>
                  {formatTokens(msg.usage) && <span className="muted">{formatTokens(msg.usage)}</span>}
                  {!msg.pending && msg.content && (
                    <button className="ghost tiny" onClick={() => copy(msg.content, "Answer copied")}>
                      Copy
                    </button>
                  )}
                </div>
              )}
            </div>
          ),
        )}
      </div>

      {pendingAck && conn && (
        <div className="ack">
          <div>
            <strong>Send to {conn.label}?</strong>{" "}
            {conn.kind === "claude_code"
              ? "Your message and this conversation will be passed to your local Claude Code, which sends them to Anthropic."
              : `Your message and this conversation will be sent to ${providerName(conn.kind)}${conn.kind === "openai_compatible" && conn.base_url ? ` (${conn.base_url})` : ""} for processing.`}{" "}
            {billingExplanation(connectionBilling(conn))}
          </div>
          <div className="ack-actions">
            <button className="ghost small" onClick={() => setPendingAck(null)}>
              Cancel
            </button>
            <button
              className="primary small"
              onClick={async () => {
                await api.acknowledgeConnection(conn.id).catch(() => {});
                onInfoChanged();
                send(pendingAck, true);
              }}
            >
              Send — don’t ask again for this connection
            </button>
          </div>
        </div>
      )}

      {sendError && (
        <div className="send-error">
          <ErrorCard error={sendError} onOpenProviders={() => onOpenSettings("providers")} />
        </div>
      )}

      <footer className="composer">
        <textarea
          ref={inputRef}
          rows={1}
          value={input}
          placeholder={selection.model ? `Ask ${modelName(info, selection.model)}…` : "Connect a provider to start…"}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={onKeyDown}
          autoFocus
          spellCheck
        />
        {streaming ? (
          <button className="send stop" onClick={stop} title={`Stop (${m}.)`} aria-label="Stop">
            ■
          </button>
        ) : (
          <button className="send" onClick={() => send(input)} disabled={!input.trim()} aria-label="Send">
            ↑
          </button>
        )}
      </footer>
      <div className="statusline">
        {conn ? (
          <>
            <span className={`dot ${conn.status?.state === "error" ? "err" : conn.status?.state === "ok" ? "ok" : ""}`} />
            <span>
              Sends to <strong>{conn.label}</strong> · {billingLabel(connectionBilling(conn), conn.account?.auth_detail)}
            </span>
          </>
        ) : (
          <span className="muted">No model selected</span>
        )}
      </div>

      {pickerOpen && (
        <ModelPicker
          info={info}
          current={selection}
          onSelect={(s) => {
            setSelection(s);
            setPickerOpen(false);
            focus();
          }}
          onClose={() => {
            setPickerOpen(false);
            focus();
          }}
          onOpenSettings={() => {
            setPickerOpen(false);
            onOpenSettings("providers");
          }}
        />
      )}
    </div>
  );
});
