# Phase 5 – Qualitätssicherung & Testen

## Automatisierte Tests

```bash
npm ci && npm run build          # Frontend (wird in die App eingebettet)
cargo test --workspace           # Rust: Core + App-Shell
npm test                         # Frontend (vitest)
npm run typecheck
cargo clippy --workspace --all-targets -- -D warnings
```

Optional, gegen die **echten** Anbieter-Endpunkte mit absichtlich ungültigen
Schlüsseln (kostenlos, kein Konto nötig):

```bash
CUE_LIVE=1 cargo test -p cue-core --test live_invalid_credentials -- --nocapture
```

### Abdeckung

| Bereich | Tests |
|---|---|
| SSE-Parser | geteilte Chunks, CRLF, Kommentare, Mehrzeilen-Daten, unterminierte Events |
| Anthropic | Request-Format (Header, System, max_tokens, Rollen), Streaming inkl. ausgeblendeter Thinking-Blöcke, Usage, Modell; Fehler 401/404/429/529, Credit-Balance → Quota, Prompt zu lang; Fehler mitten im Stream; Stream ohne Abschluss; Pagination der Modellliste |
| OpenAI (API-Key) | Responses-API-Request, Streaming, Usage, `response.incomplete` (max tokens), `error`-Event (Rate-Limit), 401 `invalid_api_key`, Modellfilter (keine Embeddings/Whisper/…) |
| ChatGPT-Plan | dokumentierte Pflichtfelder (`store:false`, `stream:true`, keine verbotenen Felder), `response.failed` mit `subscription_sharing_usage_limit_exceeded` → Planlimit-Hinweis, Modellkatalog (`visibility`, Serverreihenfolge) |
| Sign in with ChatGPT | PKCE gemäß RFC 7636, Authorize-URL mit allen dokumentierten Parametern (Erst- und Re-Auth), Code-Tausch, ID-Token-Prüfung mit RSA-Testschlüssel: falsche Nonce, falsche Audience, manipulierte Signatur, fehlender Plan-Scope, gefälschter `state`, `access_denied`, fehlende ausgegebene `client_id`; Refresh mit Rotation und terminalen Refresh-Fehlern; echter Loopback-Callback-Server |
| Gemini | Request-Format (Rollen `user/model`, systemInstruction, maxOutputTokens), Streaming mit ausgeblendeten Thought-Teilen, Usage inkl. Thinking-Tokens; Fehler `API_KEY_INVALID`, `NOT_FOUND`, `RESOURCE_EXHAUSTED`, `PERMISSION_DENIED`; Safety-Block; Modellfilter |
| OpenAI-kompatibel | System-Nachricht, Streaming, `[DONE]`, Usage und vom Anbieter gemeldete Kosten |
| Claude Code | stream-json-Parser (keine doppelte Ausgabe), Fehler „Not logged in“ → Auth-Hinweis, Mehrfach-Turn-Prompt, End-to-End gegen ein Fake-`claude`-Skript inkl. Aufräumen der System-Prompt-Datei |
| Allgemein | Abbruch während einer Anfrage, Netzwerkfehler (retrybar), fehlende Konfiguration |
| Secrets | Chunking unter dem Windows-Limit, Überschreiben/Löschen räumt alle Teile auf |
| Einstellungen | Modi nutzen nur verfügbare Modelle (inkl. deaktivierter Verbindung, verschwundenem Modell), Datei-Roundtrip ohne Secrets, Dateirechte 0600, Defaults |
| App-Shell | Validierung neuer Verbindungen (kein Claude-OAuth, https-Pflicht außer localhost), Hotkey-Syntax und System-Konflikte, Kontextaufbau (fehlgeschlagene Antworten ausgelassen, Retry ohne Duplikat) |
| Frontend | sicherer Markdown-Parser (kein HTML), Hotkey-Aufnahme und -Anzeige je Plattform |

## Manuelle End-to-End-Prüfung (durchgeführt unter Linux/Xvfb)

Die echte App (Debug-Build) wurde unter einem virtuellen Display mit
gnome-keyring (Secret Service) gestartet und per `xdotool` bedient:

- Erststart öffnet „AI Providers“; Keyring wird erkannt.
- Verbindung mit ungültigem Schlüssel → Fehlerkarte „Authentication problem“;
  Schlüssel liegt nur im Secret Service (`secret-tool`), in keiner Datei.
- Schlüssel ersetzt → „Connected · 2 models“.
- Erste Nachricht → Datenschutzhinweis „Send to …?“; danach Streaming mit
  Markdown, Codeblock, Attribution, Abrechnungs-Badge und Token-Zahlen.
- Folgefrage nach Moduswechsel per `Ctrl+3` → anderes Modell, voller Kontext
  beim Server angekommen.
- Rate-Limit-Antwort → Fehlerkarte mit „Retry“.
- `Esc` versteckt das Fenster, `Ctrl+Space` holt es zurück (zentriert auf dem
  Monitor, Fokus im Eingabefeld); zweite Instanz holt das Fenster ebenfalls.
- Hotkey-Aufnahme: `Alt+Tab` → „Reserved by the system“; `Ctrl+Shift+K`
  übernommen, alter Hotkey freigegeben, neuer aktiv und gespeichert.
- Dark Mode, Modell-Picker (`Ctrl+K`), Modi mit Hinweis „Not available“, wenn
  kein passendes Modell existiert.
- Claude-Code-Verbindung mit Fake-CLI: Status „signed in (Claude subscription
  (max) via Claude Code)“, Streaming, Attribution „default → claude-sonnet-test“,
  korrekte CLI-Flags, Arbeitsverzeichnis bleibt leer.
- Sign in with ChatGPT: Authorize-URL enthält exakt die dokumentierten
  Parameter; gefälschter Callback wird abgewiesen; echter Code-Tausch gegen
  `auth.openai.com` mit ungültigem Code → `invalid_grant` sauber angezeigt.
- Verlauf: Opt-in schreibt `history.json` (0600), Opt-out löscht die Datei.
- „Remove“ (zweistufige Bestätigung) löscht die Verbindung und ihren Eintrag im
  Secret Service.
- Eingabeprüfung: `http://` zu einem fremden Host wird abgelehnt.

Hilfsmittel dafür liegen in `scripts/dev/`:

```bash
python3 scripts/dev/mock_openai_server.py 18080 /tmp/cue-requests.log
# In Cue: Add provider → "Local model server" → http://127.0.0.1:18080/v1
# Fake Claude Code: AI Providers → Claude Code (local) → Pfad scripts/dev/fake-claude
```

## Noch nicht geprüft (erfordert Hardware/Konten)

- **Windows und macOS** wurden in dieser Umgebung nicht ausgeführt (nur Linux).
  Die CI (`.github/workflows/ci.yml`) baut und testet auf beiden; das
  Verhalten von Fenster, Fokus, Tray und Hotkey muss dort manuell geprüft
  werden – Checkliste unten.
- Erfolgreiche Antworten mit **echten** API-Schlüsseln von Anthropic, OpenAI und
  Google.
- Vollständiger **ChatGPT-Login** mit Plan-Freigabe.
- **Claude Code** mit echtem Pro/Max-Login.

### Checkliste für Windows/macOS

1. Installer installieren, App starten → erscheint im Tray/Menüleiste, kein Dock-Symbol (macOS).
2. `Option+Space` (macOS) bzw. `Ctrl+Space` (Windows) öffnet/schließt die Palette; Fokus liegt im Eingabefeld.
3. Hotkey auf eine von einer anderen App belegte Kombination setzen → Fehlermeldung, alter Hotkey bleibt aktiv.
4. Schlüssel hinzufügen → in „Schlüsselbundverwaltung“ bzw. „Anmeldeinformationsverwaltung“ unter `dev.cue.desktop` sichtbar; App neu starten → Verbindung funktioniert weiter.
5. Antwort kopieren (`⌘⇧C` / `Ctrl+Shift+C`) und in einer anderen App einfügen.
6. Klick außerhalb → Fenster verschwindet (abschaltbar); `Esc` schließt.
7. „Launch at login“ an/aus.
8. Ungültiger Schlüssel, nicht verfügbares Modell (z. B. altes Modell manuell),
   Netzwerk aus (WLAN trennen) → jeweils verständliche Fehlerkarte.
