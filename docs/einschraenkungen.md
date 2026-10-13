# Offene technische Einschränkungen

Stand: 13.10.2026, Version 0.1.0.

## Recht & Freigaben

1. **Claude-Code-Brücke – rechtliche Einordnung offen.** Anthropic erlaubt
   ausdrücklich, dass Endnutzer sich mit ihrem Abo in die unveränderte
   Claude-Code-Binärdatei einloggen, auch wenn eine Plattform sie startet.
   Für das *Anbieten* von Claude Code in Produkten nennt Anthropic jedoch die
   Commercial Terms. Vor einer öffentlichen Verteilung bei Anthropic
   nachfragen; bis dahin opt-in und für die persönliche Nutzung gekennzeichnet.
   Details: [abonnements.md](abonnements.md#claude-code-brücke-bedingungen).
2. **ChatGPT-Plan-Nutzung nur für Open-Source/lokal ohne Freigabe.** Wird Cue
   kostenpflichtig oder gehostet angeboten, muss OpenAI über das
   Interessensformular zustimmen. Cue ist MIT-lizenziert und öffentlich;
   eine spätere kommerzielle oder gehostete Variante bräuchte diese
   Freigabe. Außerdem ist das Feature bei OpenAI als
   *Preview* gekennzeichnet und kann sich ändern.
3. **Claude Pro/Max lässt sich nicht direkt (ohne Claude Code) nutzen** –
   das ist eine Vorgabe von Anthropic, keine technische Lücke.
4. **Google AI Pro/Ultra lässt sich nicht nutzen** (Vorgabe von Google).

## Nicht mit echten Konten verifiziert

5. Erfolgreiche Antworten mit echten Schlüsseln von Anthropic, OpenAI, Gemini –
   getestet sind die dokumentierten Wire-Formate per Mock-Server sowie echte
   Fehlerantworten mit ungültigen Schlüsseln.
6. Vollständiger ChatGPT-Login inkl. Plan-Freigabe, Refresh und Revocation
   gegen das echte Konto – getestet sind Protokoll, Validierung und der echte
   Token-Endpunkt (mit ungültigem Code).
7. Claude Code mit echtem Abo – getestet mit einem Fake-CLI, das die
   dokumentierten Ausgabeformate erzeugt. Ältere Claude-Code-Versionen ohne
   `--include-partial-messages`, `--tools` oder `--no-session-persistence`
   schlagen mit einer verständlichen Fehlermeldung fehl.

## Plattformen

8. **Windows und macOS** konnten in dieser Umgebung nicht ausgeführt werden;
   Fenster, Fokus, Tray, Hotkey und Keychain-Integration sind nur unter Linux
   (X11/Xvfb) praktisch getestet. CI baut/testet alle drei Plattformen.
9. **Code-Signing/Notarisierung** ist vorbereitet, aber ohne Zertifikate
   nicht aktiv: unsignierte macOS-Builds lösen Gatekeeper-Warnungen aus,
   unsignierte Windows-Builds SmartScreen-Warnungen. Bei unsignierten
   macOS-Builds kann die Keychain bei jedem neuen Build nach Erlaubnis fragen.
10. **Automatische Updates** sind noch nicht aktiviert. Das Tauri-Updater-Plugin
    erfordert ein Signaturschlüsselpaar und einen Update-Endpunkt (siehe
    [veroeffentlichung.md](veroeffentlichung.md)).
11. **Linux/Wayland:** globale Hotkeys funktionieren nur unter X11/XWayland
    (Einschränkung des `global-hotkey`-Crates). Linux ist kein Zielsystem,
    wird aber gebaut.
12. Konflikte mit Hotkeys **anderer Apps** erkennt das Betriebssystem bei der
    Registrierung (Windows zuverlässig, X11 zuverlässig). macOS meldet nicht
    jede Doppelbelegung; bekannte System-Kürzel (Spotlight, Eingabequelle,
    Emoji-Picker …) prüft Cue selbst.

## Funktionen

13. **Kosten in Euro/Dollar** zeigt Cue nur, wenn der Anbieter sie im Response
    meldet (z. B. OpenRouter). Anthropic, OpenAI und Google liefern nur
    Token-Zahlen; eine eingebaute Preistabelle wurde bewusst weggelassen, weil
    sie veralten würde. Cue verlinkt stattdessen die Usage-Seiten.
14. Nur Text-Ein- und -Ausgabe; keine Bilder/Dateien, keine Werkzeuge
    (Websuche etc.), kein „Reasoning-Effort“-Regler – „Reasoning“ wählt nur ein
    passendes Modell.
15. Modi werden automatisch anhand von Modellnamen vorgeschlagen (z. B.
    „haiku“, „flash“, „mini“ → Fast). Der Vorschlag nutzt ausschließlich
    geladene, verfügbare Modelle und lässt sich je Modus überschreiben.
16. Die Modellliste der OpenAI-API wird heuristisch auf Chat-Modelle gefiltert.
17. Verlauf wird bei Opt-in **unverschlüsselt** (Dateirechte 0600) im
    App-Datenordner gespeichert.
18. Die Benutzeroberfläche ist englisch; die Dokumentation deutsch.
    Mehrsprachigkeit ist nicht umgesetzt.
19. Links in Antworten werden angezeigt, aber bewusst nicht klickbar gemacht.
20. Lange Unterhaltungen werden nicht automatisch gekürzt; bei Überschreitung
    des Kontextfensters erscheint ein Hinweis („Conversation too long“).
