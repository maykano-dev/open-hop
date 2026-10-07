# Changelog

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
