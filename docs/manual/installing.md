# Installing NightPost

## On a Mac

NightPost needs macOS 26 (Tahoe) or later, on Apple silicon.

1. Download `NightPost-<version>-macos-arm64.zip` from the [releases page](https://github.com/falklandtim/NightPost/releases).
2. Open the zip and drag **NightPost** into your Applications folder.
3. The first time, **right-click** NightPost and choose **Open**, then **Open** again. NightPost isn't signed by an Apple-registered developer, so macOS won't open it with a double-click until you've done this once.

The first time NightPost uses a saved password, macOS asks whether it may use the Keychain. Choose **Always Allow**, so it doesn't ask again.

To update, quit NightPost, then replace it in Applications with the new version.

## On Linux, as a Flatpak

The Flatpak is the easiest way. It works on any distribution, needs nothing else installed, and brings its own copy of the libraries NightPost uses.

1. Make sure Flatpak and Flathub are set up. Most distributions have them; otherwise see [flathub.org/setup](https://flathub.org/setup).
2. Download `nightpost-<version>-x86_64.flatpak` from the [releases page](https://github.com/falklandtim/NightPost/releases).
3. Install it:

   ```sh
   flatpak install --user nightpost-<version>-x86_64.flatpak
   ```

   The first time, this also downloads the GNOME runtime from Flathub, which is several hundred megabytes. Later updates are small.
4. Start **NightPost** from your app menu, or run `flatpak run co.timson.NightPost`.

To update, install the new `.flatpak` file the same way.

**Moving from a copy you built yourself.** If NightPost is already installed with `scripts/install-linux.sh`, the Flatpak copies its accounts, mail and settings across the first time it starts, just once. Quit the other NightPost first, so nothing is changing while it's copied. Your passwords stay where they are, in your keyring, and both copies use them. After that the two copies are separate: each keeps its own mail and syncs with the server.

**What the Flatpak can see.** It runs in a sandbox. It can read your Downloads, Documents and Desktop folders, and save to Downloads. When you attach a file or use Save As, your desktop's own file dialog lets you pick any file or folder. NightPost keeps its own files in `~/.var/app/co.timson.NightPost/`.

## On Linux, from the source code

If you have the source code, you can build NightPost yourself. This takes about ten minutes, and needs Rust and some development packages:

- **Fedora:** `sudo dnf install openssl-devel dbus-devel pkgconf-pkg-config gtk4-devel webkitgtk6.0-devel libsoup3-devel hunspell-en`
- **Arch (and Omarchy):** `sudo pacman -S --needed openssl dbus pkgconf gtk4 webkitgtk-6.0 hunspell-en_us`

Then, in the source folder:

```sh
scripts/install-linux.sh
```

This installs NightPost to `~/.local/bin/nightpost`, with an entry in your app menu. Run it again to update.

## Wayland and X11

On Linux, NightPost runs natively on Wayland. To run it through XWayland instead, start it with `NIGHTPOST_X11=1` set.
