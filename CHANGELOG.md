# Changelog

## v0.9.1

**Update every computer** (protocol 12).

### Added
- **The island has a lane of its own** on Windows and Linux: a thin strip across the top of the screen that maximized windows leave free, so the island is always visible and never covers a browser tab or a title bar. Where the system can't keep the strip free, it rests as a thin line as before.
- **Real app icons** in the Open tab, from every computer.
- **GNOME on Wayland**: OpenHop installs a small GNOME Shell helper (active after logging out and back in once) so it can see that computer's windows and the pointer: the island opens on hover there, the lane works, the Open tab shows what's open, and the computer's windows appear in the Control Center (clicking one switches to it; Wayland can't show it live elsewhere).

### Fixed
- **The island now closes by itself** when the pointer leaves it, after a click (e.g. on play) and after dragging files to it; dropping files lands with an animation and folds into the "sending" pill.
- **Open apps were missing**: minimized apps and apps on other desktops count as open (taskbar on Windows, Dock on macOS, the window list on Linux).
- **A computer that locks itself after a while unused no longer locks the others.** Only a lock done by hand, on the computer being used, locks them all.
- Media: cleaner app names ("Firefox", not "Mozilla firefox_…") and no repeated names.


## v0.9.0

**Update every computer** (protocol 11).

### Added
- **Media controls** in the island: what's playing on any computer (Spotify, a video in the browser, music and video apps), with cover art, a progress bar, play/pause, previous, next and shuffle, controllable from any computer. On a Mac: Spotify and Music.
- **A task manager for every computer** in the island's **Open** tab: open apps (dock or taskbar), apps running in the background with their memory use, and all apps. Switch to, quit (or force quit) and open apps on any computer.
- **Live pills**: files on their way with a progress ring that turns into a tick, a song starting, a copy from the clipboard history, an app opening.
- **When an app doesn't open**, the island says so and why.
- Tray: **Clipboard History** and **Apps on Every Computer**.
- A log file (`openhop.log`, next to the settings) to send with bug reports.

### Fixed
- **The island no longer gets in the way.** On Windows and Linux it rests as a thin line that clicks pass through (browser tabs under it work) and opens only after the pointer rests on it, or when files are dragged to it. Notices go away on their own. Hovering no longer makes it flicker.
- **MacBooks with a notch:** the island wraps around the notch instead of hiding things behind it; on other Macs it sits inside the menu bar.
- **The island in the middle of the screen** on Linux with Wayland: it's now placed at the top.
- **Apps opened from the island didn't start** on some Linux systems (OpenHop's own libraries leaked into them, e.g. from the AppImage).
- **OpenHop not working after the computers were switched off and on**: if it can't start right away (the desktop or network isn't ready yet), it keeps trying, and the start-at-login entry is refreshed every time, so it points at the current copy of the app.

### Changed
- **Shortcuts without letters**: Ctrl+Alt+Space (island) and Ctrl+Alt+Shift+Space (keep the pointer here) instead of Ctrl+Alt+V, O and L, which clashed with AltGr characters (ó, ł…) and with Paste Special. With the island open, H, C, S and O switch tabs.
- **Find My Pointer is removed** (the button, the tray item and shaking the mouse).


## v0.8.0

**Update every computer** (protocol 10). v0.7.0 was never published; its changes are part of this release.

### Added
- **Send with OpenHop** in the right-click menu of Explorer (and its **Send to** menu), Finder (a Quick Action), Files, Dolphin, Nemo, Thunar and Caja. Choose a computer, **All Computers** or **Put on the Shelf**; OpenHop doesn't have to be open. The menu keeps itself up to date with your computers.
- **Clipboard history** for every computer, in the island's **Clipboard** tab or with **Ctrl+Alt+V**: search it, copy an item again with a click or Enter, pin items to keep them.
- **The shelf**: drop files on the island to keep them reachable from every computer; click an item from another computer to copy it here.
- **Drop files on the island** to send them: while you drag, it opens with your computers as targets.
- **Open apps on any computer** (island **Open** tab, **Ctrl+Alt+O**): search the apps of every computer and open one where it lives; the pointer goes there.
- **Arrange by moving the mouse**: push the pointer toward each computer in turn instead of dragging screens.
- Click another computer in the island's **Home** tab to move the pointer there.

### Fixed
- A computer that joins now sees the others' status, apps and shelf straight away, instead of after up to half a minute.


## v0.7.0

**Update every computer** (protocol 9).

### Added
- **The island.** A small black pill at the top of the screen (around the notch on a MacBook, under the top bar on Linux) shows where the pointer is and live hints: Focus on, a computer running low, files on their way. Hover it or press **Ctrl+Alt+Space** and it springs open into the **Control Center**:
  - **Focus** for every computer at once (Do Not Disturb). Switching Do Not Disturb on by hand on one computer does the same.
  - **Find pointer**: rings close in on the pointer, on whichever screen it's on. Shaking the mouse does the same.
  - **Lock all** and **Sleep all**. Locking one computer locks the others; unlocking one wakes the others' displays.
  - Every computer's **battery and storage** at a glance, and which one has the pointer.
  - The **windows of your other computers**: click one to open it here (this replaces the separate window dock).
- **"Plug it in" warnings**: when a laptop drops to 20%, 10% and 5%, a notice appears on the screen you're using, whichever computer that is.
- **Live activities**: notifications (files received, links copied) now appear in the island instead of the corner.
- **Shortcuts**: Ctrl+Alt+Shift+arrow keys jump to the next screen that way; Ctrl+Alt+Shift+L keeps the pointer on the screen it's on (press again to release). They work from any computer's keyboard.
- **Game guard**: while a full-screen game or video is in front, the pointer doesn't slip off the screen edge and the island hides.
- **Edge comfort**: the pointer rests a moment against an edge before hopping, corners never hop, and pushing past another computer's edge needs a little push.
- The tray menu has Control Center, Focus, Find My Pointer and Lock All.
- The main window shows **Your Computers** with battery and storage, a **Focus** button, and a sliding tab control.

## v0.6.0

**Update every computer** (protocol 8).

### Added
- **Every computer controls every other.** No more *Control Others* / *Be Controlled*: move any computer's pointer off a screen edge and it carries on onto the next screen with that computer's keyboard. A laptop's touchpad can go to the desktop, and the desktop's mouse to the laptop. Whichever mouse you touch takes over.
- **One pointer.** A screen the pointer has left hides its own pointer until its own mouse or touchpad moves.
- **Live windows maximize like a native app**: the window's own maximize button (or a double click on its title bar) fills the screen it's shown on, and the app lays itself out for that size. Restore works the same way.
- Arrange the screens from any computer; every computer shows the pairing code.

### Fixed
- **Dragging a live window back** didn't work on some setups: OpenHop now uses the mouse button it pressed itself instead of asking the system.
- **Keys and clicks waited behind live window pictures.** Pictures are sent in small pieces and each connection keeps only a short queue of unsent data, so typing stays instant while a window updates.
- **Scrolling a live window** is smoother: scroll steps are combined, and big changes are sent quickly first and sharp a moment later.

### Changed
- Pairing: tap a computer in Nearby and type its code; the computer you type the code on joins it. *Forget* on a joined computer makes it stand alone again. With a shared passphrase instead of pairing, computers agree among themselves.
- A computer whose keyboard and mouse can't be captured (Wayland) can still have its screen used by the others.

## v0.5.0

**Update every computer** (protocol 6).

### Changed
- **Live windows look and act like the real window.** They show the window's own title bar and borders, with no second frame around them. Drag the title bar to move it, click its buttons, drag an edge to resize it.
- **Drag a live window back.** Carry it by its title bar off the screen edge and it goes home (still following your mouse there), or on to another computer.
- **Clicks in live windows work** when the window belongs to a Linux computer sharing its keyboard and mouse. Before, the first click could leave that computer's mouse stuck for up to 20 seconds.
- **Typing on other computers is instant on Wi-Fi.** Wi-Fi adapters doze between packets to save power and held keystrokes for up to a few hundred milliseconds; OpenHop now keeps the connection awake while you use another computer, and the Linux installer turns Wi-Fi power saving off (NetworkManager).
- **Typing into a live window goes straight to the real window** from the moment it opens (it sometimes went through the slow path and dropped letters).
- **Connect once.** OpenHop remembers that it's on: it starts by itself at sign-in, keeps running with its window closed, and stays on after restarts until you switch it off in its window or tray menu (*Turn OpenHop Off*).
- **No more toggles.** ⌃/⌘ swapping, clipboard and files, notifications, Wake-on-LAN, dragging windows across, dark mode and Do Not Disturb sync, best picture quality and starting at sign-in are always on. Only the transfer **Speed Limit** remains. Settings now have **General** and **Look** tabs.
- **The port is automatic.** If an older OpenHop holds it, that copy is closed and the port taken over; if another program has it, OpenHop quietly uses the next free one. The Port field is gone.
- **Light and dark follow your computer's setting** (no separate switch).
- The Nearby radar shows only the blue pulsing circles.

### Removed
- Battery saver: everything works the same on battery.
- Arrival sounds and their volume.

## v0.4.1

**Update every computer** (protocol 5).

### Changed
- **Live windows are much faster.** Only the parts of a window that change are sent (typing a letter sends a few hundred bytes instead of the whole window), pictures are taken through shared memory on Linux, compressed with a faster encoder, and up to two updates travel at once. Right after a click or key press OpenHop looks for the change immediately.
- **A window you drag (or open) on another computer moves there.** The original is hidden on its own computer (it keeps running so you can use it) and comes back when you close it on the other side. Windows and Linux; macOS doesn't allow it.
- **Live windows look like the real thing**: same size as the original, its own title, sharp text (full colour detail), no grey bars. Resizing it resizes the real window, and the other way round.
- **A new Nearby radar**: device icons, soft sonar waves, and a moving line to the computers you're connected to.
- The **Address** shown for pairing leaves out container and virtual machine networks (Docker's 172.x addresses).

## v0.4.0

**Update every computer.** v0.4 uses protocol 4.

### Added
- **Live windows from any computer.** Press **Ctrl+Alt+Space** (or use the tray menu) for a dock that lists the open windows of every connected computer. Click one to open it live on the computer you're at: it updates as it changes, and your clicks, scrolling and typing go to the real window. Dragging a window by its title bar off the screen edge does the same, opening it under your pointer on the other computer.
- **Dark mode sync**: switching dark mode on one computer switches the others.
- **Do Not Disturb sync**: while one computer is presenting, in a full-screen call or in Do Not Disturb, the others hold their notifications too.
- **Battery saver**: live windows pause on a laptop below 20% that isn't charging.
- **Arrival sounds** (swoosh, pop or chime, with volume) and a **landing animation** when files or windows arrive.
- **Themes**: light, dark or automatic, and six accent colours.
- **Open at Login** (on by default): OpenHop starts in the background when you sign in. Closing the window always keeps it running in the tray.
- `openhop windows` lists this computer's windows (and can save a picture of one).

### Changed
- Linux: while a live window of this computer is focused on another one, your keyboard types straight into the real window.

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
