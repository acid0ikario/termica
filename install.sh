#!/bin/sh
# Compila e instala Térmica: servicio (root), app y extensión de GNOME.
set -e
cd "$(dirname "$0")"
SUDO=sudo; [ "$(id -u)" = 0 ] && SUDO=

if [ "$(id -u)" = 0 ]; then
    echo "Run ./install.sh as your regular user (without sudo); it requests sudo when needed."
    exit 1
fi

# Dependencias para compilar
missing=""
command -v cargo >/dev/null || missing="$missing cargo rustc"
pkg-config --exists gtk4 2>/dev/null || missing="$missing libgtk-4-dev"
pkg-config --exists libadwaita-1 2>/dev/null || missing="$missing libadwaita-1-dev"
command -v cc >/dev/null || missing="$missing build-essential"
if [ -n "$missing" ]; then
    echo "Missing packages. Install them with:"
    echo "  sudo apt install$missing"
    exit 1
fi
command -v nvidia-smi >/dev/null || echo "Warning: NVIDIA driver not found; GPU limits will not be applied."
[ -d /sys/devices/platform/msi-ec ] || modprobe -n msi_ec 2>/dev/null || echo "Warning: msi_ec is unavailable; fan control will not work."

echo "→ Building (the first build takes a few minutes)"
cargo build --release --locked

echo "→ Service"
$SUDO install -Dm755 target/release/termicad     /usr/local/libexec/termicad
$SUDO install -Dm644 data/termicad.service       /etc/systemd/system/termicad.service
$SUDO install -Dm644 data/com.ikario.Termica1.conf /usr/share/dbus-1/system.d/com.ikario.Termica1.conf
$SUDO install -Dm644 data/com.ikario.termica.policy /usr/share/polkit-1/actions/com.ikario.termica.policy

echo "→ App"
$SUDO install -Dm755 target/release/termica      /usr/local/bin/termica
$SUDO install -Dm644 data/com.ikario.Termica.desktop /usr/local/share/applications/com.ikario.Termica.desktop
$SUDO install -Dm644 data/com.ikario.Termica.svg /usr/local/share/icons/hicolor/scalable/apps/com.ikario.Termica.svg
$SUDO gtk-update-icon-cache -q /usr/local/share/icons/hicolor 2>/dev/null || true

# Restos de la versión en Python
if [ -d /usr/local/lib/termica ]; then
    $SUDO rm -rf /usr/local/lib/termica
fi

# El servicio viejo hacía lo mismo; se desactiva (el script se conserva como respaldo)
if systemctl is-enabled -q msi-fan-curve.service 2>/dev/null; then
    echo "→ Disabling msi-fan-curve.service (replaced by termicad)"
    $SUDO systemctl disable msi-fan-curve.service
fi

$SUDO systemctl daemon-reload
$SUDO systemctl reload dbus 2>/dev/null || true
$SUDO systemctl enable termicad.service
$SUDO systemctl restart termicad.service

echo "→ GNOME extension"
EXT=termica@ikario
DEST="$HOME/.local/share/gnome-shell/extensions/$EXT"
rm -rf "$DEST"; mkdir -p "$DEST"
for f in metadata.json extension.js i18n.js es.json metrics.js prefs.js prefsui.js stylesheet-dark.css stylesheet-light.css icons schemas; do
    cp -r "extension/$EXT/$f" "$DEST/"
done
glib-compile-schemas "$DEST/schemas"
gnome-extensions enable $EXT 2>/dev/null || echo "   (will activate after you sign in again)"

echo "Done."
