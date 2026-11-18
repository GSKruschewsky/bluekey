# Phone control

Use this PC's keyboard and mouse on your Android phone, with one shortcut to switch:

- **Super+M**: the keyboard and mouse now type and point on the phone
- **Super+Esc**: they're back on the PC

The phone stays connected in the background (like a Bluetooth keyboard that's always on), so switching is instant. It's
a Linux take on Samsung's Multi Control, and needs nothing installed on the phone: to Android, the PC is just a
Bluetooth keyboard and mouse.

Tested on Ubuntu 26.04 (GNOME 50, Wayland), BlueZ 5.85, an Intel Bluetooth adapter, with a Galaxy S22+. Other recent
distributions and Android phones should work the same way.

## Requirements

- A Bluetooth adapter that can act as a Bluetooth LE peripheral: `bluetoothctl show` lists `Roles: peripheral`
- BlueZ 5.x (`bluetoothctl --version`)
- Rust 1.85 or newer (`rustup` or your distribution's `cargo`), plus the D-Bus headers:
  - Debian/Ubuntu: `sudo apt install build-essential pkg-config libdbus-1-dev`
  - Fedora: `sudo dnf install gcc pkgconf-pkg-config dbus-devel`
  - Arch: `sudo pacman -S base-devel dbus`
- GNOME, for the automatic shortcut setup. On other desktops everything works the same, you just bind the
  `phone-control` command to a key yourself.

## Setup

### 1. Build

```bash
git clone https://github.com/GSKruschewsky/bluekey.git ~/bluekey
cd ~/bluekey
cargo build --release
```

Build as your normal user, not with sudo.

### 2. Install the daemon

```bash
sudo ./phone-control/install-system.sh
```

This:

- copies `bluekeyd` and `bluekey` to `/usr/local/bin`
- creates a `bluekey` system user in the `input` group (it reads your keyboard and mouse, without root)
- installs and enables `bluekeyd.service`, so it runs from boot and the phone can always reconnect
- installs a D-Bus policy that lets only **your** user control bluekeyd (the user you ran sudo from)

### 3. Pair the phone over Bluetooth LE

This is the only fiddly part. The phone has to pair with the PC over *Bluetooth LE*, which is what keyboards like this
use. Most PCs also speak *classic* Bluetooth, and Android prefers classic when both are on, which gives a pairing
with no keyboard or mouse in it. So turn classic Bluetooth off just while pairing:

```bash
# If the phone was paired with this PC before, unpair it on the phone too, then:
bluetoothctl devices Paired                 # find the phone's address
bluetoothctl remove <PHONE-ADDRESS>         # only if it's listed

# Classic Bluetooth off for a moment (this briefly disconnects headphones etc.)
sudo btmgmt power off && sudo btmgmt bredr off && sudo btmgmt power on
sudo btmgmt bondable on
bluetoothctl pairable on
```

On the phone: **Settings → Connections → Bluetooth**, scan, and tap your PC. It shows up under the PC's Bluetooth name,
with a keyboard icon. If either side asks to confirm a code, confirm it on both.

Then put classic Bluetooth back, and restart Bluetooth once so the phone is listed under its permanent address
(right after pairing it can show up under a temporary random one):

```bash
sudo btmgmt power off && sudo btmgmt bredr on && sudo btmgmt power on
bluetoothctl pairable off
sudo systemctl restart bluetooth && sudo systemctl restart bluekeyd
```

Wait a few seconds for the phone to reconnect, then:

```bash
bluekey list --long                          # your phone, with "Keyboard, Mouse"
bluetoothctl trust <PHONE-ADDRESS>
bluetoothctl info <PHONE-ADDRESS> | grep Connected   # Connected: yes
```

### 4. Phone settings

- In the Bluetooth settings, tap the gear next to your PC and keep **Input device** on.
- Keep the phone's own keyboard working: since a keyboard is now always connected, Android hides the on-screen
  keyboard unless you tell it not to.
  - Samsung: **Settings → General management → Physical keyboard → Show on-screen keyboard**
  - Pixel and most others: **Settings → System → Keyboard → Physical keyboard → Use on-screen keyboard**

### 5. Shortcut

```bash
./phone-control/install-user.sh
```

This:

- creates `~/.config/bluekey/phone-control.conf` and fills in the phone, keyboard and mouse when there's only one
  candidate for each. Check it: see [phone-control.conf.example](phone-control.conf.example) for what goes where.
- links `~/.local/bin/phone-control` to this repo's script (so `git pull` updates it)
- adds a GNOME shortcut for it, **Super+M** by default, after checking nothing else uses that key

GNOME uses Super+M (and Super+V) to open its notification list, so the first run will say Super+M is taken. To give
Super+M to phone control and keep Super+V for the notification list:

```bash
gsettings set org.gnome.shell.keybindings toggle-message-tray "['<Super>v']"
./phone-control/install-user.sh
```

If something else already uses Super+V (a clipboard extension, say), use `"[]"` instead. The clock in the top bar
still opens the list either way, and `gsettings reset org.gnome.shell.keybindings toggle-message-tray` undoes it.
Or pick another key: `./phone-control/install-user.sh --binding '<Super><Alt>p'`. To see whether a key is free
first: `./phone-control/install-user.sh --check '<Super><Alt>p'`.

### 6. Try it

Press **Super+M**: a "Controlling phone" notification appears, and the keyboard and mouse now work on the phone.
**Super+Esc** brings them back ("Back on the PC"). The phone never sees Super+Esc, so whatever is on its screen (a 2FA
code, a message) stays there while you type it on the PC.

To change the way back: `bluekey escape-shortcut LEFTCTRL+LEFTALT+ESC` (evdev key names joined with `+`). This lasts
until bluekeyd restarts.

## Updating

```bash
cd ~/bluekey && git pull
cargo build --release
sudo ./phone-control/install-system.sh
```

The phone drops off for a few seconds while bluekeyd restarts, then reconnects by itself.

## Troubleshooting

**The shortcut does nothing.** GNOME silently refuses a shortcut that's taken, including by extensions:

```bash
journalctl --user -b | grep "Failed to grab accelerator"
./phone-control/install-user.sh --check '<Super>m'
```

**"Phone not connected"**

- `systemctl status bluekeyd`: it has to be running
- `bluetoothctl info <PHONE-ADDRESS> | grep Connected`: if it's `no`, turn Bluetooth off and on on the phone
- check **Input device** is still on in the phone's settings for this PC

**Nothing happens on the phone even though it's connected.** Check the config's `PHONE` matches `bluekey list`, and
look for errors: `journalctl -u bluekeyd -b`. If the phone was paired with an older bluekey (or over classic
Bluetooth), unpair on both sides and pair again (step 3).

**Keys feel stuck on the PC or the phone.** Press and release the stuck key (Super, usually). If the keyboard stays
grabbed after something crashed: `sudo systemctl restart bluekeyd` releases everything.

**The phone's on-screen keyboard disappeared.** See step 4.

**After suspend, or turning Bluetooth off and on.** bluekeyd advertises again by itself; give the phone a few seconds.

## Security and privacy

- bluekeyd runs as the unprivileged `bluekey` user, but it can read every keyboard and mouse, and send them to a
  paired device. That's why the D-Bus policy lets only your user give it commands.
- Don't leave debug logging on (`RUST_LOG=debug` in the service): it writes every key report, so everything you type
  on the phone, to the system journal.
- Only make the PC pairable (`bluetoothctl pairable on`) while you're pairing.

## Uninstall

```bash
sudo systemctl disable --now bluekeyd
sudo rm /usr/local/bin/bluekey /usr/local/bin/bluekeyd /etc/systemd/system/bluekeyd.service \
        /usr/share/dbus-1/system-services/us.colbystuff.Bluekey.service /etc/dbus-1/system.d/us.colbystuff.Bluekey.conf
sudo userdel bluekey
rm ~/.local/bin/phone-control
rm -r ~/.config/bluekey
```

Then delete the "Phone control" shortcut in **Settings → Keyboard → View and Customize Shortcuts → Custom Shortcuts**,
and unpair the PC on the phone.
