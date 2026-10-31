#!/bin/sh
# Quita Térmica y vuelve a activar el servicio anterior (msi-fan-curve).
set -e
SUDO=sudo; [ "$(id -u)" = 0 ] && SUDO=
gnome-extensions disable termica@ikario 2>/dev/null || true
rm -rf "$HOME/.local/share/gnome-shell/extensions/termica@ikario"
$SUDO systemctl disable --now termicad.service 2>/dev/null || true
$SUDO rm -f /etc/systemd/system/termicad.service /usr/share/dbus-1/system.d/com.ikario.Termica1.conf \
    /usr/share/polkit-1/actions/com.ikario.termica.policy /usr/local/bin/termica /usr/local/libexec/termicad \
    /usr/local/share/applications/com.ikario.Termica.desktop \
    /usr/local/share/icons/hicolor/scalable/apps/com.ikario.Termica.svg
$SUDO rm -rf /usr/local/lib/termica
$SUDO systemctl daemon-reload
[ -f /etc/systemd/system/msi-fan-curve.service ] && $SUDO systemctl enable --now msi-fan-curve.service
echo "Térmica uninstalled. Your profiles remain in /etc/termica/profiles.json"
