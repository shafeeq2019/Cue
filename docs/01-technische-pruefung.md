# Phase 1 – Technische Prüfung

Stand der Prüfung: **13. Oktober 2026**. Alle Aussagen stammen aus der jeweils
offiziellen Dokumentation der Anbieter (Links unten). Wo die Doku etwas offen
lässt, ist das ausdrücklich vermerkt. Nutzungsbedingungen ändern sich – vor
jeder Veröffentlichung erneut prüfen.

## Ergebnis auf einen Blick

| Anbieter | Abo nutzbar in Cue? | Offiziell erlaubter Weg | In Cue umgesetzt |
|---|---|---|---|
| **Anthropic Claude Pro/Max** | **Nein, nicht direkt.** Drittanbieter-Apps dürfen keinen Claude.ai-Login anbieten. | API-Schlüssel (Claude Console). Zusätzlich: die *unveränderte* Claude-Code-CLI des Nutzers, in die er sich selbst einloggt. | API-Schlüssel ✅ · Claude-Code-Brücke ✅ (opt-in) |
| **OpenAI ChatGPT Plus/Pro** | **Ja** – über das offizielle „Sign in with ChatGPT“ mit Plan-Nutzung (Preview für Open-Source/lokale Apps). | OAuth 2.0 + PKCE im System-Browser, Loopback-Redirect, Scope `chatgpt.tokens.use.direct`. | ✅ |
| OpenAI API | – | API-Schlüssel (platform.openai.com), Abrechnung pro Token. | ✅ |
| **Google AI Pro/Ultra** | **Nein.** Abo-Vorteile gelten nur in der AI-Studio-Weboberfläche. | Gemini-API-Schlüssel (AI Studio), Free Tier oder Cloud Billing. | API-Schlüssel ✅ |
| Weitere (OpenRouter, Groq, Mistral, Ollama, LM Studio …) | – | Offizielle OpenAI-kompatible APIs bzw. lokale Server. | ✅ |

## Anthropic Claude

**Quelle:** Claude Code – *Legal and compliance*, Abschnitt „Authentication and
credential use“ – <https://code.claude.com/docs/en/legal-and-compliance>

Wörtlich (Auszug):

> „**OAuth authentication** is intended exclusively for purchasers of Claude
> Free, Pro, Max, Team, and Enterprise subscription plans and is designed to
> support ordinary use of Claude Code and other native Anthropic applications.“
>
> „**Developers** building products or services that interact with Claude's
> capabilities, including those using the Agent SDK, should use API key
> authentication through Claude Console or a supported cloud provider.
> Anthropic does not permit third-party developers to offer Claude.ai login
> into their own applications, or to route requests through Free, Pro, or Max
> plan credentials on behalf of their users. Moreover, developers may not
> collect, store, or intermediate Claude.ai credentials or session tokens —
> sign-in to a Claude account must complete through Anthropic's own flow.“
>
> „[…] Nor does it prevent an end user from signing in to the unmodified Claude
> Code binary with their own Claude subscription, including where a platform
> hosts Claude Code as described under *Can customers offer Claude Code in
> their products?* above.“

Konsequenzen für Cue:

1. **Kein „Mit Claude anmelden“ in Cue.** Ein eigener OAuth-Flow gegen
   Claude.ai, das Auslesen von Claude-Code-Tokens (Keychain,
   `~/.claude/.credentials.json`), Session-Cookies oder `claude setup-token`
   werden **nicht** verwendet.
2. **Claude API per API-Schlüssel** (separates Konto, Abrechnung pro Token) ist
   der offiziell vorgesehene Weg für Drittanbieter-Apps.
3. **Claude-Code-Brücke (opt-in):** Cue kann die lokal installierte,
   *unveränderte* `claude`-CLI als Kindprozess starten (`claude -p … --output-format stream-json`).
   Der Nutzer meldet sich ausschließlich in Claude Code selbst an
   (`claude auth login`, Anthropics eigener Flow). Cue sieht, liest, speichert
   oder übermittelt keine Claude-Zugangsdaten. Die zitierte Passage erlaubt
   ausdrücklich, dass Endnutzer sich mit ihrem Abo in die unveränderte
   Claude-Code-Binärdatei einloggen – auch wenn diese von einer anderen
   Plattform gestartet wird. Bedingungen und Einschätzung siehe
   [abonnements.md](abonnements.md#claude-code-brücke-bedingungen).
   Hinweis: Die Doku nennt für das *Anbieten* von Claude Code in Produkten die
   Commercial Terms als Voraussetzung. Ob das für eine kostenlose, lokal
   laufende Open-Source-App gilt, ist **nicht abschließend geklärt** – daher ist
   die Funktion opt-in, klar gekennzeichnet und als offener Punkt dokumentiert.

Technische Details der Claude API (aus der offiziellen API-Referenz):
`POST /v1/messages` mit `x-api-key`, `anthropic-version: 2023-06-01`, Streaming
per SSE (`message_start`, `content_block_delta`/`text_delta`, `message_delta`,
`message_stop`, `error`), `max_tokens` Pflicht; Modellliste über
`GET /v1/models` (paginiert, `has_more`/`last_id`, inkl. `max_tokens` und
`max_input_tokens`). Fehlertypen: `authentication_error` (401),
`permission_error` (403), `not_found_error` (404), `rate_limit_error` (429),
`overloaded_error` (529), `billing_error`, `invalid_request_error` (u. a.
„credit balance is too low“).

## OpenAI / ChatGPT

**Quellen:**
- Übersicht Plan-Nutzung für Open-Source-Apps – <https://developers.openai.com/siwc/token-sharing-open-source>
- Registrierung & Anmeldung – <https://developers.openai.com/siwc/token-sharing-open-source/sign-in>
- Konten, Token-Refresh, Abmeldung – <https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions>
- Modelle & Inferenz – <https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference>
- Fehler & Wiederherstellung – <https://developers.openai.com/siwc/token-sharing-open-source/errors-and-recovery>
- Preview-Einschränkungen – <https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations>
- OIDC-Discovery – <https://auth.openai.com/.well-known/openid-configuration>

Kernaussagen:

- „*These docs explain ChatGPT plan usage for open-source and locally hosted
  apps. If you're interested in offering it in a paid or remotely hosted app,
  complete the interest form.*“ → Für eine **kostenlose, lokal laufende
  Open-Source-App** ist der Weg dokumentiert und ohne Partner-Client-ID nutzbar
  (dynamische Registrierung). Für kostenpflichtige oder gehostete Apps ist eine
  Freigabe durch OpenAI nötig.
- Flow: Authorization Code + PKCE (S256) im System-Browser,
  `client_id=dynamic_agent_client` beim ersten Login (danach die ausgegebene
  `client_id`), `agent_name_hint`, `ext_agent_host_id`, Redirect
  `http://127.0.0.1:<port>/auth/callback`, Scopes
  `openid profile email offline_access resource.invoke chatgpt.tokens.use.direct`,
  `resource=https://api.openai.com/v1`.
- ID-Token gegen JWKS prüfen (Signatur, Issuer, Audience, Ablauf, Nonce);
  Inferenz erst, wenn `chatgpt.tokens.use.direct` gewährt wurde.
- Inferenz: `POST https://api.openai.com/v1/responses` mit
  `store: false`, `stream: true`; Felder wie `temperature`,
  `max_output_tokens` werden auf dieser Route abgelehnt. Modellliste über
  `GET /v1/models` (Feld `models`, nur `visibility == "list"`).
- Refresh: `grant_type=refresh_token` mit ausgegebener `client_id` und
  `resource`; rotierende Refresh-Tokens, Refreshes serialisieren.
  Abmeldung: Revocation-Endpoint.
- Plus-Nutzer teilen sich das 5-Stunden-Limit über alle Apps; Verwaltung unter
  <https://chatgpt.com/settings/usage>.
- API-Schlüssel (platform.openai.com) werden unabhängig davon pro Token
  abgerechnet und sind **nicht** Teil eines ChatGPT-Abos.

## Google Gemini

**Quelle:** <https://ai.google.dev/gemini-api/docs/google-ai-plans>

> „Google AI plan benefits for developer usage apply only within the Google AI
> Studio web interface.“
>
> „Direct use of the Gemini API (such as using API keys or external
> applications) is billed and managed separately.“

→ Google AI Pro/Ultra lassen sich in Cue **nicht** nutzen. Gemini wird per
API-Schlüssel aus Google AI Studio angebunden (`x-goog-api-key`,
`POST /v1beta/models/{model}:streamGenerateContent?alt=sse`, Modellliste
`GET /v1beta/models`, Fehlerstatus `UNAUTHENTICATED`/`INVALID_ARGUMENT` +
`API_KEY_INVALID`, `PERMISSION_DENIED`, `NOT_FOUND`, `RESOURCE_EXHAUSTED`).
Die Gemini-CLI-Anmeldung („Login with Google“) gehört zu Googles eigenem
Werkzeug und wird nicht nachgebaut.

## Live-Verifikation (13.10.2026)

Mit absichtlich ungültigen Schlüsseln gegen die echten Endpunkte (keine Kosten,
kein Konto nötig) – siehe `crates/cue-core/tests/live_invalid_credentials.rs`:

| Endpunkt | Antwort | Von Cue erkannt als |
|---|---|---|
| `api.anthropic.com/v1/models` & `/v1/messages` | 401 „invalid x-api-key“ | `auth` |
| `api.openai.com/v1/models` & `/v1/responses` | 401 `invalid_api_key` | `auth` |
| `generativelanguage.googleapis.com` (Liste & Stream) | 400 `API_KEY_INVALID` | `auth` |
| `auth.openai.com/.well-known/jwks.json` | 4 RSA-Schlüssel | alle in Prüfschlüssel umwandelbar |
| `auth.openai.com/api/accounts/oauth/token` (Fake-Code aus der App) | 400 `invalid_grant` | `auth` – Request-Format akzeptiert |

Nicht verifizierbar ohne echte Konten: erfolgreiche Antworten mit gültigen
Schlüsseln, ein vollständiger ChatGPT-Login mit Plan-Freigabe und Claude Code
mit echtem Abo. Diese Pfade sind gegen die dokumentierten Formate mit
Mock-Servern getestet (siehe [testen.md](testen.md)).
