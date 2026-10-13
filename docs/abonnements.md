# Welche Abonnements kann Cue nutzen?

Kurzfassung: **ChatGPT Plus/Pro ja** (offizielles „Sign in with ChatGPT“).
**Claude Pro/Max nicht direkt**, aber über die eigene, unveränderte Claude-Code-
Installation des Nutzers. **Google AI Pro/Ultra nein.** API-Schlüssel werden
immer separat pro Token abgerechnet – kein Chat-Abo deckt API-Kosten ab.

| Verbindung in Cue | Was wird abgerechnet | Anzeige in Cue |
|---|---|---|
| Anthropic Claude (API-Schlüssel) | Claude-API-Konto (platform.claude.com), pro Token | „Paid API usage“ |
| Claude Code (lokal) | Was Claude Code nutzt: bei Anmeldung mit Claude.ai die Planlimits des Pro/Max-Abos, bei API-Schlüssel die API-Abrechnung | „Claude plan via Claude Code“ bzw. „API billing via Claude Code“ – laut `claude auth status` |
| ChatGPT plan (Sign in with ChatGPT) | Nutzungslimits des ChatGPT-Plans (Plus: 5-h-Limit geteilt mit anderen Apps) | „ChatGPT plan“ |
| OpenAI API (API-Schlüssel) | OpenAI-API-Konto, pro Token | „Paid API usage“ |
| Google Gemini (API-Schlüssel) | Gemini API (Free Tier oder Cloud Billing) | „Paid API usage“ |
| OpenAI-kompatibel mit Schlüssel (OpenRouter, Groq, …) | jeweiliger Dienst | „Paid API usage“ |
| Lokaler Server ohne Schlüssel (Ollama, LM Studio) | nichts | „Local / self-managed“ |

Jede Antwort zeigt Verbindung, Anbieter, angefragtes und tatsächlich
bedienendes Modell, Abrechnungsart und – wenn der Anbieter sie liefert –
Token-Zahlen. Kosten in US-Dollar werden nur angezeigt, wenn der Anbieter sie
selbst im Response mitliefert (z. B. OpenRouter); Cue erfindet keine Preise.
Für die eigentlichen Kosten verlinkt jede Verbindung auf die Usage-Seite des
Anbieters.

## Claude Pro/Max – warum kein „Mit Claude anmelden“?

Anthropics offizielle Regel (Claude Code Legal & Compliance, Stand 13.10.2026):
Drittanbieter dürfen **keinen Claude.ai-Login in ihre Apps einbauen**,
**keine Anfragen über Free/Pro/Max-Zugangsdaten** im Namen ihrer Nutzer leiten
und **keine Claude.ai-Zugangsdaten oder Session-Tokens sammeln, speichern oder
vermitteln**. Entwickler sollen API-Schlüssel verwenden. Deshalb implementiert
Cue ausdrücklich **nicht**:

- einen eigenen OAuth-Flow gegen Claude.ai,
- das Auslesen von Claude-Code-Tokens aus Keychain oder `~/.claude`,
- `claude setup-token`-Tokens als Bearer-Token in eigenen API-Aufrufen,
- das Nachbauen interner claude.ai-Web-Schnittstellen oder Cookies.

## Claude-Code-Brücke: Bedingungen

Dieselbe Seite sagt aber auch, die Regel hindere **nicht** daran, dass sich ein
Endnutzer mit seinem eigenen Abo in die **unveränderte Claude-Code-Binärdatei**
einloggt – auch wenn eine Plattform Claude Code startet. Cue nutzt genau das:

- Cue startet die vom Nutzer installierte `claude`-CLI als Kindprozess
  (`claude -p --output-format stream-json …`). Die Binärdatei wird nicht
  verändert, keine Anmeldemethode entfernt.
- Die Anmeldung erfolgt ausschließlich in Claude Code über Anthropics eigenen
  Flow. Cue liest nur `claude auth status`, um anzuzeigen, **wie** Claude Code
  angemeldet ist (Abo, API-Schlüssel, Cloud-Anbieter).
- Alle Werkzeuge sind deaktiviert (`--tools ""`), keine MCP-Server, keine
  gespeicherten Sitzungen – Claude Code dient als reiner Chat.
- Die Verbindung ist opt-in und in der UI mit den Bedingungen beschrieben.

**Offener Punkt (bewusst nicht wegdiskutiert):** Für das *Anbieten* von Claude
Code in Produkten verlangt Anthropic die Commercial Terms und dass keine
Nutzung im Namen der Endnutzer bezahlt oder weiterverkauft wird. Außerdem
gelten die beworbenen Pro/Max-Limits für „ordinary, individual usage of Claude
Code and the Agent SDK“. Ob ein kostenloses, lokal laufendes Open-Source-
Werkzeug, das die CLI des Nutzers startet, unter diese Produkt-Regel fällt, sagt
die Doku nicht eindeutig. Empfehlung vor einer öffentlichen Verteilung: bei
Anthropic (Sales/Legal) nachfragen. Bis dahin: Funktion für die persönliche
Nutzung, klar gekennzeichnet; sie lässt sich mit einer Zeile in
`src/labels.ts` (Option entfernen) und `validate_new` in
`src-tauri/src/commands.rs` deaktivieren.

## ChatGPT Plus/Pro – Sign in with ChatGPT

OpenAI dokumentiert die Plan-Nutzung ausdrücklich für **Open-Source- und lokal
gehostete Apps** (dynamische Registrierung ohne Partner-Client-ID). Für
**kostenpflichtige oder gehostete** Apps ist OpenAIs Interessensformular
vorgesehen. Daraus folgt: Wird Cue als kostenloses Open-Source-Projekt
verteilt, ist der Weg offiziell vorgesehen; bei kommerzieller Verteilung muss
vorher eine Freigabe von OpenAI eingeholt werden. Die Lizenzentscheidung für
Cue ist noch offen (siehe [einschraenkungen.md](einschraenkungen.md)).

Ablauf in Cue: „Continue with ChatGPT“ → Browser öffnet chatgpt.com → Nutzer
meldet sich dort an und genehmigt die Plan-Nutzung → Redirect auf
`127.0.0.1:1455` → Cue tauscht den Code (PKCE), prüft das ID-Token gegen
OpenAIs JWKS und den Scope `chatgpt.tokens.use.direct` → Tokens in den
OS-Credential-Store. Verbrauch und App-Zugriff verwaltet der Nutzer unter
<https://chatgpt.com/settings/usage>.

## Google AI Pro/Ultra

Laut Google gelten die Abo-Vorteile nur in der AI-Studio-Weboberfläche; die
Gemini API (Schlüssel oder externe Apps) wird separat abgerechnet. Cue bietet
deshalb nur den API-Schlüssel an.
