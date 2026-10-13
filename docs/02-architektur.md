# Phase 2 – Architektur

## Tech-Stack-Entscheidung: Tauri 2 + Rust + React/TypeScript

| Anforderung | Tauri 2 (gewählt) | Electron | Native (Swift + WinUI) |
|---|---|---|---|
| Globale Hotkeys Win/macOS | offizielles Plugin (`RegisterHotKey`, Carbon, X11) | `globalShortcut` | je Plattform separat |
| Startzeit / RAM | System-WebView, Binary ≈ 10–15 MB, ≈ 40–80 MB RAM | eigenes Chromium, ≈ 150+ MB RAM | sehr gut |
| Sichere Secrets | Rust-Backend + `keyring` (Keychain / Credential Manager / Secret Service) | Node + keytar/safeStorage | nativ |
| Secrets vom UI fernhalten | natürliche Trennung: HTTP-Aufrufe nur im Rust-Prozess | Main-Prozess, aber Node im Bundle | ja |
| Streaming / Provider-Code | einmal in Rust, testbar ohne UI | einmal in JS | zweimal schreiben |
| Installer & Updates | `.dmg/.app`, `.msi`, NSIS; Updater-Plugin; GitHub-Action | electron-builder | je Plattform |
| Aufwand | eine Codebasis | eine Codebasis | zwei Codebasen |

**Begründung:** Tauri erfüllt die Kernziele „schnell öffnen, wenig Speicher,
sichere Speicherung“ am besten und hält alle Zugangsdaten im nativen
Rust-Prozess. Das UI ist eine reine Darstellungsschicht ohne Netz- oder
Secret-Zugriff (strikte CSP: `connect-src ipc:` – die WebView kann keine
externen Server erreichen).

## Komponenten

```
┌───────────────────────── Desktop-Oberfläche (React, src/) ─────────────────────────┐
│ Chat-Palette · Modell-Picker · AI Providers · Einstellungen · Verlauf              │
└───────────────▲───────────────────────────────────────────────▲────────────────────┘
                │ Tauri-Commands (invoke)                       │ Channel (Streaming-Events)
┌───────────────┴──────────── App-Shell (Rust, src-tauri/) ─────┴────────────────────┐
│ window.rs   Schwebendes Fenster, Positionierung, Esc/Blur                           │
│ hotkey.rs   Globaler Hotkey-Manager + Konflikterkennung                             │
│ commands.rs Provider-/Modellverwaltung, Auth-Flows, Einstellungen                   │
│ chat.rs     Einheitliche Chat-Orchestrierung, Token-Refresh, Abbruch                │
│ history.rs  Verlauf (nur im Speicher, außer Opt-in)                                 │
│ state.rs    Zustand, Diagnoseprotokoll (redigiert), Sitzungsverbrauch              │
└───────────────▲─────────────────────────────────────────────────────────────────────┘
                │
┌───────────────┴──────────── cue-core (Rust, crates/cue-core/) ──────────────────────┐
│ providers/  Provider-Trait + anthropic, openai, gemini, openai_compatible,          │
│             claude_code                                                             │
│ chatgpt_oauth.rs  Sign in with ChatGPT (PKCE, Loopback, JWKS-Prüfung, Refresh)      │
│ secrets.rs  OS-Credential-Store (+ Chunking für das Windows-Größenlimit)            │
│ settings.rs Einstellungen (ohne Secrets), Modi-Auflösung                            │
│ error.rs    Einheitliches Fehlermodell + Redaktion                                  │
│ sse.rs      SSE-Parser mit Abbruch & Idle-Timeout                                   │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

Zuordnung zu den geforderten Komponenten:

1. **Desktop-Oberfläche** – `src/`
2. **Globaler Hotkey-Manager** – `src-tauri/src/hotkey.rs`
3. **Provider- und Modellverwaltung** – `src-tauri/src/commands.rs`, `cue-core/settings.rs`
4. **Authentifizierungsmodul** – `cue-core/chatgpt_oauth.rs`, `cue-core/secrets.rs`, Claude-Code-Status in `providers/claude_code.rs`
5. **Einheitliche KI-Schnittstelle** – `cue-core/providers/mod.rs` (`Provider`-Trait, `StreamEvent`, `ChatOutcome`)
6. **Einstellungen & lokale Daten** – `cue-core/settings.rs`, `src-tauri/src/history.rs`
7. **Fehlerbehandlung & Diagnose** – `cue-core/error.rs`, Diagnose-Tab (`state.rs`)

## Provider-Schnittstelle

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn list_models(&self) -> CueResult<Vec<ModelInfo>>;
    async fn chat(&self, req: &ChatRequest, tx: &EventTx, cancel: &CancellationToken)
        -> CueResult<ChatOutcome>;
}
```

- `ChatRequest` – Modell, abwechselnde User/Assistant-Nachrichten, optionaler
  System-Prompt, optionales Token-Limit.
- `StreamEvent` – `Delta`, `Model` (tatsächlich bedienendes Modell), `Usage`,
  `Notice` – für alle Anbieter identisch.
- `ChatOutcome` – Stop-Grund (`complete`, `max_tokens`, `refusal`, `other`) und
  Verbrauch.
- `CueError` – `kind` (auth, permission_denied, model_not_found, rate_limited,
  quota_exceeded, subscription_limit, overloaded, context_too_long, network,
  timeout, cancelled, not_configured, …), verständliche Meldung, Hinweis,
  redigierte Details, HTTP-Status, Request-ID, `retryable`.

**Neuen Anbieter hinzufügen:** Modul in `cue-core/src/providers/` anlegen,
`Provider` implementieren, eine Zeile in `build_provider` und eine
`ProviderKind`-Variante ergänzen, in `src/labels.ts` eine Option eintragen.
Viele Anbieter funktionieren bereits ohne Code über „Other provider
(OpenAI-compatible)“.

## Authentifizierungskonzept

| Methode | Wo stattfindet | Was Cue speichert |
|---|---|---|
| API-Schlüssel | Eingabe in Cue (write-only, nie wieder angezeigt) | Schlüssel im OS-Credential-Store |
| Sign in with ChatGPT | System-Browser auf chatgpt.com; Cue empfängt nur den Redirect auf `127.0.0.1` | Token-Satz (JSON) im OS-Credential-Store; `client_id` und E-Mail (keine Secrets) in den Einstellungen |
| Claude Code | ausschließlich in Claude Code (`claude auth login`) | nichts – Cue startet nur die CLI |
| Lokaler Server ohne Schlüssel | – | nichts |

Cue verlangt **nie** Passwörter. Mehrere Konten pro Anbieter werden als
getrennte Verbindungen mit eigenem Namen geführt („Work“, „Personal“); der
Wechsel erfolgt über den Modell-Picker.

## Sicherheitsmaßnahmen

- Secrets nur im OS-Credential-Store (macOS Keychain, Windows Credential
  Manager, Linux Secret Service). Lange Werte werden für das 2560-Byte-Limit
  von Windows in Teilstücke zerlegt. Ist kein Store verfügbar, bleiben Secrets
  nur im Arbeitsspeicher – nie im Klartext auf der Platte.
- Kein Command gibt Secrets an die WebView zurück; das UI kennt nur `has_secret`.
- Strikte CSP; die WebView kann keine externen Hosts kontaktieren. Externe
  Links nur über eine Allowlist (Provider-Konsolen).
- Modellantworten werden nie als HTML gerendert (eigener sicherer
  Markdown-Parser → React-Elemente); Links in Antworten sind nicht klickbar.
- OAuth: PKCE S256, zufälliger `state`/`nonce` pro Versuch, Prüfung von
  `state`, Signatur, Issuer, Audience, Ablauf, Nonce, Konto-Identität bei
  Re-Auth; Loopback-Listener nur auf `127.0.0.1`; Refresh serialisiert;
  Revocation beim Entfernen.
- Fehlertexte und Diagnoseprotokoll werden redigiert (Schlüsselmuster wie
  `sk-…`, `AIza…`, JWTs). Nachrichteninhalte werden nie protokolliert.
- Einstellungs- und Verlaufsdateien werden atomar mit Rechten `0600` (Unix)
  geschrieben.
- Basis-URLs eigener Server: nur `https://`, `http://` ausschließlich für
  `localhost`.
- Claude Code läuft mit `--tools ""`, `--strict-mcp-config`,
  `--disable-slash-commands`, `--no-session-persistence` in einem leeren
  Arbeitsverzeichnis.

## Datenfluss einer Anfrage

1. UI → `send_message` (Modell-Referenz + Text).
2. Rust prüft: Verbindung aktiv, Modell in der zuletzt geladenen (autorisierten)
   Liste.
3. Kontext = erfolgreiche Vorgänger-Nachrichten derselben Unterhaltung.
4. Credential wird erst jetzt aus dem OS-Store gelesen (ChatGPT: ggf. Refresh).
5. Provider streamt; Events gehen per Tauri-Channel an das UI; Abbruch via
   `CancellationToken`.
6. Ergebnis wird mit Attribution (Verbindung, Anbieter, Modell, tatsächliches
   Modell, Abrechnungsart, Verbrauch) gespeichert – im Speicher bzw. bei
   Opt-in auf der Platte.
