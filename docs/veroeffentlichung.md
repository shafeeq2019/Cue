# Phase 6 – Installationspakete & Veröffentlichung

## Lokal bauen

```bash
npm ci
npm run tauri build
```

Ergebnisse unter `target/release/bundle/`:

| Plattform | Pakete |
|---|---|
| macOS | `macos/Cue.app`, `dmg/Cue_<version>_<arch>.dmg` |
| Windows | `msi/Cue_<version>_x64_en-US.msi`, `nsis/Cue_<version>_x64-setup.exe` (Installation pro Benutzer) |
| Linux | `deb/`, `appimage/` |

Universal-Build für macOS (Intel + Apple Silicon):

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
npm run tauri build -- --target universal-apple-darwin
```

Windows lädt bei Bedarf die WebView2-Runtime nach (`downloadBootstrapper`);
auf Windows 10/11 ist sie in der Regel bereits vorhanden.

## Über GitHub Actions

`.github/workflows/release.yml` baut beim Push eines Tags `v*` die Pakete für
macOS (universal) und Windows und legt einen **Entwurf** eines GitHub-Releases
mit allen Dateien an:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

### Signieren (empfohlen)

- **macOS:** Secrets `APPLE_CERTIFICATE` (Base64 des .p12),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, für Notarisierung
  `APPLE_ID`, `APPLE_PASSWORD` (app-spezifisch), `APPLE_TEAM_ID`.
- **Windows:** Zertifikat-Thumbprint in `bundle.windows.certificateThumbprint`
  oder Signierung per `signCommand` (z. B. Azure Trusted Signing) –
  siehe Tauri-Doku „Windows Code Signing“.

## Automatische Updates aktivieren (noch nicht aktiv)

1. `npm run tauri signer generate -- -w ~/.tauri/cue.key`
2. `tauri-plugin-updater` hinzufügen (`cargo add tauri-plugin-updater` in
   `src-tauri`, Plugin in `lib.rs` registrieren).
3. In `tauri.conf.json`: `bundle.createUpdaterArtifacts: true` und
   `plugins.updater` mit `pubkey` und `endpoints` (z. B. `latest.json` des
   GitHub-Releases).
4. Im Release-Workflow `TAURI_SIGNING_PRIVATE_KEY` und
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` als Secrets setzen.

Bewusst nicht vorkonfiguriert, weil ein öffentlicher Schlüssel und ein
Endpunkt des Projektinhabers nötig sind.

## Speicherorte auf dem Zielsystem

| Was | macOS | Windows |
|---|---|---|
| Einstellungen (ohne Secrets) | `~/Library/Application Support/dev.cue.desktop/settings.json` | `%APPDATA%\dev.cue.desktop\settings.json` |
| Verlauf (nur bei Opt-in) | `~/Library/Application Support/dev.cue.desktop/history.json` | `%APPDATA%\dev.cue.desktop\history.json` |
| Schlüssel & Tokens | Schlüsselbund, Dienst `dev.cue.desktop` | Anmeldeinformationsverwaltung, Einträge zu `dev.cue.desktop` |

Deinstallation entfernt die App; Einstellungen und Schlüsselbund-Einträge
bleiben erhalten. Zum vollständigen Entfernen vorher in Cue alle Verbindungen
löschen („Remove“ löscht die Secrets aus dem Credential Store) und den Verlauf
leeren.
