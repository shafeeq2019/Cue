import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, asCueError, type AppInfo } from "./api";
import { Chat, type ChatHandle } from "./components/Chat";
import { History } from "./components/History";
import { SettingsView, type Tab } from "./components/Settings";

type View = { name: "chat" } | { name: "history" } | { name: "settings"; tab: Tab };

function useTheme(theme: AppInfo["settings"]["theme"] | undefined) {
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const t = theme === "light" || theme === "dark" ? theme : mq.matches ? "dark" : "light";
      document.documentElement.dataset.theme = t;
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);
}

export function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [view, setView] = useState<View>({ name: "chat" });
  const [toastMsg, setToastMsg] = useState<string | null>(null);
  const chatRef = useRef<ChatHandle>(null);
  const toastTimer = useRef<number | undefined>(undefined);

  const toast = useCallback((m: string) => {
    setToastMsg(m);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToastMsg(null), 2600);
  }, []);

  const refresh = useCallback(() => {
    api
      .appInfo()
      .then((i) => {
        setInfo(i);
        setLoadError(null);
      })
      .catch((e) => setLoadError(asCueError(e).message));
  }, []);

  useEffect(refresh, [refresh]);

  // Startup guidance, decided from state rather than racing events.
  const startupHandled = useRef(false);
  useEffect(() => {
    if (!info || startupHandled.current) return;
    startupHandled.current = true;
    if (info.hotkey_error) {
      toast(`Global shortcut unavailable: ${info.hotkey_error}`);
      setView({ name: "settings", tab: "general" });
    } else if (info.settings.connections.length === 0) {
      setView({ name: "settings", tab: "providers" });
    }
  }, [info, toast]);
  useTheme(info?.settings.theme);

  useEffect(() => {
    const un = [
      listen("cue://shown", () => {
        refresh();
        chatRef.current?.focus();
      }),
      listen("cue://open-settings", () => setView({ name: "settings", tab: "providers" })),
    ];
    return () => un.forEach((p) => p.then((f) => f()));
  }, [refresh, toast]);

  // Esc: leave sub-pages first, then hide the window.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.preventDefault();
      if (view.name !== "chat") {
        setView({ name: "chat" });
        chatRef.current?.focus();
      } else {
        api.hideWindow();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

  if (!info) {
    return <div className="shell loading">{loadError ? `Cue could not start: ${loadError}` : ""}</div>;
  }

  const openSettings = (tab: string = "providers") => setView({ name: "settings", tab: tab as Tab });

  return (
    <div className="shell">
      <div className={view.name === "chat" ? "view" : "view hidden"}>
        <Chat
          ref={chatRef}
          info={info}
          onOpenSettings={openSettings}
          onOpenHistory={() => setView({ name: "history" })}
          onInfoChanged={refresh}
          toast={toast}
        />
      </div>
      {view.name === "history" && (
        <History
          info={info}
          toast={toast}
          onBack={() => setView({ name: "chat" })}
          onOpen={async (id) => {
            setView({ name: "chat" });
            await chatRef.current?.openConversation(id);
          }}
        />
      )}
      {view.name === "settings" && (
        <SettingsView
          info={info}
          initialTab={view.tab}
          toast={toast}
          onChanged={refresh}
          onBack={() => {
            setView({ name: "chat" });
            chatRef.current?.focus();
          }}
        />
      )}
      {toastMsg && (
        <div className="toast" role="status">
          {toastMsg}
        </div>
      )}
    </div>
  );
}
