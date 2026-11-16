#!/usr/bin/env bash
# Installs bluekeyd as an always-on system service, so a paired phone can reconnect at any time.
# Build first as your normal user, then run this with sudo from that same user:
#   cargo build --release
#   sudo ./phone-control/install-system.sh
# Safe to re-run: it's also how you install an update.
set -euo pipefail

REPO=$(cd "$(dirname "$0")/.." && pwd)
DESKTOP_USER=${SUDO_USER:-}

die() { echo "install-system.sh: $*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

[ "$(id -u)" -eq 0 ] || die "run it with sudo"
[ -n "$DESKTOP_USER" ] && [ "$DESKTOP_USER" != root ] ||
    die "run it through sudo from your desktop user, which becomes the only user allowed to use bluekeyd"
for bin in bluekeyd bluekey; do
    [ -x "$REPO/target/release/$bin" ] || die "missing target/release/$bin, run 'cargo build --release' first (as your user)"
done

step "Installing bluekeyd and bluekey to /usr/local/bin"
install -m 755 "$REPO/target/release/bluekeyd" "$REPO/target/release/bluekey" /usr/local/bin/

step "Creating the 'bluekey' system user (in the 'input' group, to read keyboards and mice)"
if id bluekey >/dev/null 2>&1; then
    usermod -a -G input bluekey
else
    useradd --system --no-create-home --shell "$(command -v nologin || echo /usr/sbin/nologin)" --groups input bluekey
fi

step "Installing the systemd unit and D-Bus activation file"
install -m 644 "$REPO/bluekey/install/bluekeyd.service" /etc/systemd/system/bluekeyd.service
install -m 644 "$REPO/bluekey/install/us.colbystuff.Bluekey.service" /usr/share/dbus-1/system-services/us.colbystuff.Bluekey.service

step "Installing the D-Bus policy: only '$DESKTOP_USER' may control bluekeyd"
policy=$(mktemp)
sed "s/@USER@/$DESKTOP_USER/" "$REPO/phone-control/us.colbystuff.Bluekey.conf.in" > "$policy"
install -m 644 "$policy" /etc/dbus-1/system.d/us.colbystuff.Bluekey.conf
rm -f "$policy"
systemctl reload dbus

step "Enabling and (re)starting bluekeyd"
systemctl daemon-reload
systemctl enable bluekeyd.service
systemctl restart bluekeyd.service
sleep 1
systemctl is-active --quiet bluekeyd.service ||
    die "bluekeyd didn't start, see: journalctl -u bluekeyd -b"

step "Done"
echo "bluekeyd is running and starts on boot. Next: pair your phone (phone-control/README.md, step 3)."
