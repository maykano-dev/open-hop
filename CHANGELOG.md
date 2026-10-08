# Changelog

## v0.3.3

### Fixed
- **Linux: dragging a file from the Files app (Nautilus) stopped at the screen edge.** The file manager keeps hold of the mouse during a drag (and newer versions ignore the Escape key OpenHop used to cancel it), so OpenHop couldn't take over the pointer. OpenHop now follows the pointer itself while the drag is carried to the other computer, behind an invisible window that refuses drops, so letting go drops the files on the other computer and nothing is dropped locally. Tested with GNOME Files 46.

## v0.3.2

Compatible with v0.3.x. Update every computer: the fixes are on the receiving side.

### Fixed
- **Copying an image or a video pasted its path on the other computer.** The receiving computer only put a file reference on its clipboard, which many apps paste as a path. A received file now goes on the clipboard in every format the system's apps expect (Ubuntu Files/Nautilus, Caja and Nemo need `x-special/gnome-copied-files`; Windows Explorer gets a proper copy). A copied picture file is also put on the clipboard **as the picture itself**, so it pastes into chats, documents and image editors.
- Copied files weren't picked up from file managers that end the list with a NUL byte (PCManFM/libfm).
- Copying an image in a browser could send the image's URL instead of the image.
- Linux: big clipboard images are sent and received with the X11 INCR protocol, and every image format (PNG, JPEG, BMP, GIF, WebP, TIFF) is understood.

### Changed
- **Drag & drop drops into the app under the pointer** on Linux (X11) and Windows, with the files' own names, just like a local drag. Elsewhere, or when nothing accepts the drop, the files are saved to Downloads › OpenHop and put on the clipboard to paste.
- Linux (Wayland): dragging files *from* a Wayland app is now detected.

### Added
- `openhop-app --clipboard` (or `openhop clipboard` with the CLI) saves a report of what's on the clipboard and what OpenHop would send, for troubleshooting.

## v0.3.1

Compatible with v0.3.0 (same protocol), but update every computer to get the fixes.

### Fixed
- **"Port 24850 is in use" on the host.** Starting OpenHop now closes any older copy still running in the background (single-instance protection can't see copies from earlier versions). If another program holds the port, the host falls back to the next free port and announces it, so clients still find it.
- **Hosts and clients didn't show up for pairing.** This followed from the host failing to start. The Linux installers now also open the firewall (ufw/firewalld) for discovery and connections.
- The window layout: the settings column on the left scrolls on its own, and the right side (status, radar, arrangement) stays put.

### Added
- **Nearby radar.** Computers running OpenHop appear on an animated radar, Xender/AirDrop style. Tap one to pair.
- **Connect by address.** If discovery is blocked, pair by typing the host's address (shown on the host under *Pair a Computer*) and code. CLI: `openhop client --server ADDR --pair CODE`.

## v0.3.0

**Update every computer.** v0.3 uses protocol 3, so it can't connect to v0.2 or earlier.

### Added
- **AirDrop-style pairing.** The host shows a 6-digit code; on the other computer, click **Pair** next to it and type the code. A per-device 256-bit key is saved on both sides, so no shared passphrase is needed (it still works if you prefer one). The code changes after use, and five wrong codes pause pairing for a minute.
- **No router needed.** Discovery and connections work on every network interface over IPv4 and IPv6 link-local, so two computers joined by an Ethernet, USB-C or Thunderbolt cable connect directly while each keeps its own Wi-Fi. They're shown with a **Cable** badge.
- **Transfer speed limit** for file and image transfers.
- **Check for Updates**: automatic daily check, plus one-click download and install of the right installer (.exe, .dmg, .deb, .rpm or AppImage).
- CLI: `openhop client --pair CODE`, `openhop paired [--forget NAME]`; the server prints its pairing code.

### Fixed
- **The on/off switch could fail with "port 24850 is busy".** Restarting (toggling, saving settings) now waits for the old listener to close, reuses the port immediately, and retries briefly.
- Only one copy of the app can run; opening it again brings the existing window forward.
- Saving settings in the window no longer overwrites things the engine saved meanwhile (pairings, arrangement, Wake-on-LAN details).


## v0.2.0

**Update every computer.** v0.2 uses a new protocol, so v0.1 and v0.2 computers can't connect to each other.

### Fixed
- **Switching to another computer could get stuck until you reconnected.** A computer that stopped reading (asleep, Wi-Fi napping, or a big clipboard image on slow Wi-Fi) could freeze the server's engine. When it reconnected it was also renamed (e.g. `laptop (192.168.1.5)`) and lost its place in the arrangement. Each connection now has its own writer with a priority lane for mouse and keyboard, write timeouts drop stalled computers within seconds, and a reconnect replaces the old session and keeps its position.
- Copying a file pasted its *path* on the other computer. It now pastes the file itself.
- Windows: input hooks that Windows silently removes are detected and reinstalled.
- If the mouse and keyboard can't be captured (e.g. another app holds a grab), OpenHop stays on the current computer instead of getting into a half-switched state.

### Added
- Copy & paste of files, folders and videos between any computers (client to client relays through the server). Big copies are sent when the pointer arrives.
- Drag & drop files across screens, in any direction.
- iOS-style notifications with actions: **Open**, **Show in folder**, and **Open** for links copied on another computer.
- Wake-on-LAN: move toward a sleeping computer to wake it.
- Transfers list with progress, and new switches for notifications and Wake-on-LAN.
- Large clipboard images are sent in pieces, so they never delay the pointer.

## v0.1.0
- First release: mouse and keyboard sharing across Windows, macOS and Linux, text and image clipboard sync, Noise encryption, LAN discovery, Apple-style app, one-line installers.
