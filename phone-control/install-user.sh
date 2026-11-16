#!/usr/bin/env bash
# Per-user setup: the phone-control config, the phone-control command and a GNOME shortcut for it. No sudo.
#   ./phone-control/install-user.sh                            set up, with Super+M as the shortcut
#   ./phone-control/install-user.sh --binding '<Super><Alt>p'  set up with another shortcut
#   ./phone-control/install-user.sh --check '<Super><Alt>p'    only report what already uses a shortcut
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CONFIG=${XDG_CONFIG_HOME:-$HOME/.config}/bluekey/phone-control.conf
COMMAND=$HOME/.local/bin/phone-control
MEDIA_KEYS=org.gnome.settings-daemon.plugins.media-keys
KB_PATH=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/phone-control/
KB_SCHEMA=$MEDIA_KEYS.custom-keybinding:$KB_PATH

BINDING='<Super>m'
CHECK=''
while [ $# -gt 0 ]; do
    case $1 in
        --binding) BINDING=${2:?--binding needs a shortcut, like '<Super>m'}; shift 2 ;;
        --check) CHECK=${2:?--check needs a shortcut, like '<Super>m'}; shift 2 ;;
        -h|--help) sed -n '2,5s/^# \{0,1\}//p' "$0"; exit 0 ;;
        *) echo "Unknown option: $1 (see --help)" >&2; exit 2 ;;
    esac
done

step() { printf '\n==> %s\n' "$*"; }

# "<Shift><Super>P" -> "shift+super+p", so a shortcut compares equal however it's spelled
normalize() {
    local rest=${1,,} mod mods=()
    while [[ $rest =~ ^\<([^>]+)\>(.*)$ ]]; do
        mod=${BASH_REMATCH[1]} rest=${BASH_REMATCH[2]}
        case $mod in
            primary|ctrl) mod=control ;;
            mod4) mod=super ;;
            mod1) mod=alt ;;
        esac
        mods+=("$mod")
    done
    [ -n "$rest" ] || return 1
    printf '%s+' $(printf '%s\n' "${mods[@]}" | sort -u)
    printf '%s\n' "$rest"
}

# Every setting that might hold a shortcut: GNOME's schemas, extensions' own schemas, and other custom shortcuts.
# (Extensions are easy to miss: their schemas aren't in the global list.)
all_bindings() {
    gsettings list-recursively 2>/dev/null
    local file schema path
    for file in /usr/share/gnome-shell/extensions/*/schemas/*.gschema.xml \
                "$HOME"/.local/share/gnome-shell/extensions/*/schemas/*.gschema.xml; do
        [ -e "$file" ] || continue
        # <schema id="..."> may span lines
        for schema in $(tr '\n' ' ' <"$file" | grep -o '<schema [^>]*' | grep -o ' id="[^"]*"' | cut -d'"' -f2); do
            gsettings --schemadir "$(dirname "$file")" list-recursively "$schema" 2>/dev/null
        done
    done
    for path in $(gsettings get $MEDIA_KEYS custom-keybindings 2>/dev/null | grep -o "'[^']*'" | tr -d "'"); do
        [ "$path" = "$KB_PATH" ] && continue
        echo "custom shortcut $(gsettings get "$MEDIA_KEYS.custom-keybinding:$path" name 2>/dev/null)" \
             "$(gsettings get "$MEDIA_KEYS.custom-keybinding:$path" binding 2>/dev/null)"
    done
}

# Prints the settings already using shortcut $1, nothing if it's free
conflicts() {
    local want key line token
    want=$(normalize "$1") || { echo "Not a shortcut: $1" >&2; exit 2; }
    key=${want##*+}
    all_bindings | grep -iF ">$key'" | sort -u | while IFS= read -r line; do
        for token in $(grep -o "'[^' ]*'" <<<"$line" | tr -d "'"); do
            if [ "$(normalize "$token" 2>/dev/null)" = "$want" ]; then
                echo "  $line"
                break
            fi
        done
    done
    return 0
}

if [ -n "$CHECK" ]; then
    used=$(conflicts "$CHECK")
    if [ -n "$used" ]; then
        printf '%s is already used by:\n%s\n' "$CHECK" "$used"
        exit 1
    fi
    echo "$CHECK is free"
    exit 0
fi

step "Config: $CONFIG"
if [ -e "$CONFIG" ]; then
    echo "Already exists, leaving it as it is."
else
    mkdir -p "$(dirname "$CONFIG")"
    cp "$HERE/phone-control.conf.example" "$CONFIG"
    set_option() { sed -i "s|^$1=.*|$1=$2|" "$CONFIG"; echo "  $1=$2 (detected)"; }

    # Only fill in what's unambiguous: exactly one phone, one keyboard, one mouse
    phones=$(bluekey list 2>/dev/null | tr ',' '\n' | grep -E '^([0-9A-F]{2}:){5}[0-9A-F]{2}$' || true)
    keyboards=$(ls /dev/input/by-id/ 2>/dev/null | grep -E -- '-event-kbd$' | grep -vE -- '-if[0-9]+-' || true)
    mice=$(ls /dev/input/by-id/ 2>/dev/null | grep -E -- '-event-mouse$' | grep -vE -- '-if[0-9]+-' || true)
    [ "$(grep -c . <<<"$phones")" -eq 1 ] && set_option PHONE "$phones"
    [ "$(grep -c . <<<"$keyboards")" -eq 1 ] && set_option KEYBOARD "/dev/input/by-id/$keyboards"
    [ "$(grep -c . <<<"$mice")" -eq 1 ] && set_option MOUSE "/dev/input/by-id/$mice"
    echo "Created it. Check it, and fill in anything still showing an example value:"
    grep -E '^(PHONE|KEYBOARD|MOUSE)=' "$CONFIG" | sed 's/^/  /'
fi

step "Command: $COMMAND -> $HERE/phone-control"
mkdir -p "$(dirname "$COMMAND")"
if [ -e "$COMMAND" ] && [ ! -L "$COMMAND" ]; then
    mv "$COMMAND" "$COMMAND.bak"
    echo "Moved the existing $COMMAND to $COMMAND.bak"
fi
ln -sfn "$HERE/phone-control" "$COMMAND"
echo "Linked (a git pull updates it)."

if ! gsettings list-schemas 2>/dev/null | grep -qx "$MEDIA_KEYS"; then
    step "Shortcut"
    echo "GNOME isn't running here: bind $COMMAND to a key in your desktop's keyboard settings."
    exit 0
fi

step "GNOME shortcut: $BINDING"
used=$(conflicts "$BINDING")
if [ -n "$used" ]; then
    printf '%s is already used by:\n%s\n' "$BINDING" "$used"
    echo "Free it first, or choose another with --binding (test one with --check)."
    exit 1
fi
list=$(gsettings get $MEDIA_KEYS custom-keybindings)
case $list in
    *"'$KB_PATH'"*) ;;
    "@as []"|"[]") gsettings set $MEDIA_KEYS custom-keybindings "['$KB_PATH']" ;;
    *) gsettings set $MEDIA_KEYS custom-keybindings "${list%]}, '$KB_PATH']" ;;
esac
since=$(date '+%Y-%m-%d %H:%M:%S')
gsettings set "$KB_SCHEMA" name 'Phone control'
gsettings set "$KB_SCHEMA" command "$COMMAND"
gsettings set "$KB_SCHEMA" binding "$BINDING"
sleep 2
# GNOME only reports a refused shortcut in the log
if journalctl --user --since "$since" --no-pager -o cat 2>/dev/null |
        grep -qF "Failed to grab accelerator for keybinding custom:$KB_PATH"; then
    echo "GNOME refused $BINDING: something else holds it. Choose another with --binding."
    exit 1
fi

escape=$(bluekey escape-shortcut 2>/dev/null | sed -n 's/^Keyboard shortcut is: //p')
step "Done: press $BINDING to control the phone, ${escape:-the escape shortcut} to come back"
