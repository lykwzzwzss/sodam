# SodaM Privacy Notice

Effective date: November 10, 2026

SodaM is a **local-only** music player: no sign-in, no network access, and no
telemetry. The online capabilities of early versions (accounts, cookies,
signing services, play reporting, download caches) have been removed.

## 1. Data we do not collect

SodaM does not collect or upload personal information. There is no account
system, advertising, profiling, or telemetry. The application makes no network
requests while running.

## 2. Data stored locally

SodaM only reads and writes the following on this device:

- **Configuration**: volume, play mode, theme, language, window options,
  shortcuts, detailed-log switch, and the local music folder.
  - Windows: `config.json` next to `sodam.exe`. On upgrade, settings are imported from
    `%APPDATA%\sodam\config.json` if there is no config next to the executable.
  - Linux: `~/.config/sodam/config.json`
  - macOS: `~/Library/Application Support/sodam/config.json`
- **Playback session**: the last queue, current track and position, shuffle
  history, and the playlist being browsed, written to `sodam/session.json` and
  `sodam/checkpoint.json` in the local data directory (`%LOCALAPPDATA%` on
  Windows, `~/.local/share` on Linux, `~/Library/Application Support` on macOS).
- **Artwork cache**: the original embedded cover and a pre-blurred backdrop,
  written under `sodam/artwork/` in the system cache directory. Delete the
  cache directory to clear it; it is regenerated on the next play.
- **Diagnostic logs**: detailed logging is off by default. When enabled, logs
  are written under `sodam/logs/` in the user data directory. They contain
  local playback/device/file errors only, never audio content or credentials.

Playlists come from the folder the user selects. SodaM never sends file paths,
track lists, or artwork to any server.

## 3. System integration

- Windows System Media Transport Controls (SMTC) expose the current title,
  artist, album, and playback state to the OS media panel. This stays on the
  local machine.
- macOS Now Playing and Linux MPRIS behave the same way.
- The tray icon and “close to tray” only use local process state.

## 4. Third-party components

SodaM uses open-source libraries such as rodio/Symphonia (decoding), lofty
(tags), and GPUI (UI). They run locally and are not wired to any network
feature by SodaM.

## 5. Changes

If a future version reintroduces any network capability, this notice will be
updated first and the change will be visible in the UI. The current branch
contains no HTTP client, cookies, tokens, or device-fingerprint fields.

## 6. Contact

Please use the repository issue tracker. Do not include personal music-library
paths, credentials, or other private data in issues.
