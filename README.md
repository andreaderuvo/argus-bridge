# argus-bridge

Your computer and an [Argus](https://github.com/andreaderuvo/argus) server, linked — on **macOS,
Windows and Linux**, with no SSH and nothing listening on your computer:

- **Folders in sync**, both ways, like Dropbox: real files on your disk that open with a
  double-click, work offline, and go back to the server.
- **This computer, lent to Argus** — each switched on by you: a terminal on it in a window of Argus,
  its browser for the agents on the server, a command an agent asks and you allow.
- **A port of the server on your `localhost`** (*Open on Mac* in Argus), **Argus in a window of its
  own**, and the agents' bells as notifications of your system.
- **It keeps itself up to date**, with releases signed by this project's key.

It was called argus-sync until 0.1.4: the `argus-sync` command, `argus-sync://` links and the
settings already made all keep working.

## Folders in sync

- **No administrator, no SSH key, no FUSE**: an app you install for yourself.
- **Connect with a click**: in Argus, a folder's menu → *Sync to your computer…* gives a link; it opens
  argus-bridge already pointed at that folder. You choose where it goes on your computer.
- **A key that opens one folder**: no terminal, no sessions, no other file. Taken back from Argus.
- **Nothing is lost**: a file changed on both sides is kept twice (`report (conflict from laptop …).pdf`);
  a delete goes to the trash (yours, or Argus's); a file over the size limit (500 MB by default) waits
  to be asked for.
- **Never a mass delete**: a synced folder that is gone (moved, renamed, a disk not mounted) is an
  error, never made again empty; a round that would empty the folder or delete a quarter of it at once
  (20 files or more) deletes nothing on the server and says so. Argus also refuses bulk deletes through
  a key. A folder moved with *Move on this computer…* is followed.

## This computer, connected to Argus

Besides folders, argus-bridge can connect the computer it runs on to an Argus server — with no SSH
and nothing listening here: one WebSocket from here to Argus, the road the sync already takes.
What it lends is three switches, **all off until you turn them on**:

- **Browser** — a Chrome of its own (its own profile: none of your sessions), driven by the
  agents on the server through an address only they are given.
- **Terminal** — a shell on this computer, in a window of Argus, for you.
- **Actions** — a command an agent asks to run here; each one is put to you in Argus as Do it /
  No, and its output goes back to the agent.

In Argus: Settings → Computers → *Connect a computer…* gives an `argus-bridge://computer?…` link;
open it here. From a terminal: `argus-bridge computer '<link>' --browser --terminal --actions`.
Disconnecting in Argus closes everything this computer lent.

- A question for you (*Do it / No*) arrives as a notification of the system, answered on it (macOS).
- With Terminal or Actions on and no **Full Disk Access**, macOS stops an agent's command at each
  protected folder to ask "Allow?": the *This computer* card says so and opens the setting.
- A Chrome left on the browser profile by an earlier run is taken over, or ended and started again.
- **Open Argus** opens Argus in a window of the app.

## Cross-OS, by design

| | handled |
|---|---|
| `Report.pdf` and `report.pdf` on the server, a case-insensitive disk here | one is synced, the other is reported |
| `è` written by a Mac (NFD) and by Linux (NFC) | the same file |
| names Windows refuses (`: * ? " < > \|`, `CON`, `NUL`, trailing dot…) | reported, never renamed silently |
| file times kept at different precisions (APFS, NTFS, exFAT) | each side compared with itself only |
| a file open in another program (Windows) | retried on the next round |
| `.DS_Store`, `Thumbs.db`, `desktop.ini`, Office/LibreOffice lock files | ignored everywhere |

## Install

Download the installer for your system from the
[latest release](https://github.com/andreaderuvo/argus-bridge/releases/latest):

| system | file | first launch |
|---|---|---|
| macOS (Apple silicon and Intel) | `.dmg` | signed with the project's own certificate, not Apple's: right-click it → **Open** → **Open**, once (macOS 15: *System Settings → Privacy & Security → Open Anyway*). Every version carries the same signature, so an **Always allow** holds across updates |
| Windows 10/11 | `.exe` (or `.msi`) | SmartScreen may warn: **More info** → **Run anyway** |
| Linux | `.AppImage`, `.deb` or `.rpm` | `chmod +x` the AppImage, or install the package |

No administrator rights are needed on macOS (drag it into your own Applications) or with the
Windows `.exe`. Then, in Argus, open a folder's menu → **Sync to your computer…** and click the link.

**Updates** are offered in a dialog — *Update and restart* or *Later* — unless *Install updates by
themselves* is ticked at the foot of the window; *Check for updates* is there and in the menu. Only
versions signed by the project's key are installed. The **language** follows the system's, or the one
chosen at the foot of the window (English, Italiano, Español, Français).

It needs an Argus with the sync routes (`/api/sync/*`), from the release after 2026-10-15 onwards.

## Security

- A sync key opens **one folder** and nothing else: it cannot reach a terminal, a session, the
  settings or another folder; read-only unless you allow writing when you make it. Argus keeps only a
  hash of it, and you can take it back from the same menu at any time.
- The key travels with every request. Over plain `http` on a network you do not trust, anyone on it
  can read it: use Argus over `https` (`tls_cert`), a VPN such as Tailscale, or an SSH tunnel.
- Writes obey the server's upload limit (`max_upload_bytes`); deletes made through a key go to a trash
  beside Argus's config, never straight to nothing.

## Layout

```
crates/core   the engine: names.rs (cross-OS rules), plan.rs (what to do — pure, tested case by case),
              engine.rs (doing it, safely), remote.rs (Argus's /api/sync), local.rs, state.rs, pair.rs
crates/cli    argus-bridge without a window: connect, list, run, once, fetch, pause, resume, forget
app/          the app (Tauri 2): src-tauri/ the Rust side (tray, deep links argus-bridge:// and argus-sync://, the worker),
              ui/ the window — plain HTML/JS, no bundler; ui/mock.js lets it run in a browser
tests/ui      the window in headless Chromium, against the mock
```

## Build and test

```bash
cargo test -p argus-sync-core                  # unit tests + against a real Argus (ARGUS_SRC=<argus checkout>)
cargo build --release -p argus-sync-cli        # target/release/argus-bridge
python -m pytest -q tests/ui                   # the window (needs Chromium)
cd app/src-tauri && cargo build                # the app (Linux: WebKitGTK 4.1 dev files)
```

The installers (`.dmg`, `.msi`/`.exe`, AppImage/`.deb`/`.rpm`) are built by CI on each system.

## License

MIT — see [LICENSE](LICENSE).
