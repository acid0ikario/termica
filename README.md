# Térmica

Perfiles térmicos para laptops **MSI** con gráfica **NVIDIA** en **Ubuntu / GNOME**.
Con un clic cambias la velocidad máxima de la GPU, la potencia del CPU, el Turbo Boost y la curva de los
ventiladores, y ves temperaturas, RPM, relojes y memoria en vivo desde la barra superior.

![App Térmica](docs/app.png)

## Qué incluye

| Parte | Qué hace |
|---|---|
| **Servicio `termicad`** (Rust, root, systemd) | Aplica los perfiles al hardware y expone una API en el bus de sistema (`com.ikario.Termica1`). Los reaplica al arrancar, al salir de suspensión, cuando la GPU despierta y si la NVIDIA pierde su límite. Usa unos 10 MB de RAM. |
| **App Térmica** (Rust, GTK4 + libadwaita) | Crea, edita, duplica y borra perfiles. Incluye un editor de curva de ventilador en el que arrastras los puntos, y las temperaturas en vivo. |
| **Extensión de GNOME Shell** | Icono en la barra superior con los datos que elijas (temperaturas, RPM, relojes, VRAM, RAM, consumo…), cambio de perfil, Cooler Boost y Turbo Boost. |

Cada perfil controla:

- **GPU NVIDIA:** velocidad máxima del núcleo y de la memoria (vía NVML, la librería oficial de NVIDIA).
- **CPU Intel:** potencia sostenida y de ráfaga (PL1/PL2, vía RAPL) y Turbo Boost.
- **Ventiladores MSI:** modo automático de fábrica o curva personalizada (CPU y GPU), Cooler Boost y modo del firmware (Eco/Comfort/Turbo).

Vienen tres perfiles de ejemplo: **Silencioso**, **Balanceado** y **Fábrica**.

### Barra superior

Desde la app (botón del termómetro) o desde el menú del icono eliges qué mostrar, cada dato con su icono:

![Elegir qué mostrar](docs/barra-app.png)

La ventana de preferencias de la extensión incluye una vista previa:

![Preferencias de la extensión](docs/preferencias.png)

## Compatibilidad

Probado en:

- **MSI Stealth 15M B12UE** (placa MS‑15B1, firmware del EC `15B1EMS1.102`)
- Ubuntu 26.10, GNOME Shell 51, GTK 4.24, libadwaita 1.10
- Driver NVIDIA 580 (RTX 3060 Laptop)

> **Importante:** la curva de ventilador se escribe directamente en el controlador embebido (EC) de la
> laptop, y sus direcciones dependen del modelo. Por seguridad, el servicio **solo escribe la curva si el
> firmware empieza con `15B1EMS1`**. En otros modelos funcionan el resto de controles (GPU, CPU, modo de
> ventilador y Cooler Boost vía `msi-ec`), pero no la curva personalizada.
>
> Si verificaste que tu modelo usa las mismas direcciones (`0x6A`/`0x72` CPU, `0x82`/`0x8A` GPU), puedes
> permitirlo bajo tu propio riesgo con `sudo touch /etc/termica/allow-unknown-ec`.

## Requisitos

- Ubuntu con GNOME Shell 51 (la extensión declara esa versión).
- Driver propietario de NVIDIA (`nvidia-smi`).
- Módulos del kernel `msi_ec` y `ec_sys`. El servicio los carga solo; `ec_sys` se carga con `write_support=1`.
- **Secure Boot desactivado**, o el kernel sin *lockdown*: con lockdown no se puede escribir en el EC.
- Para compilar:

```sh
sudo apt install cargo rustc build-essential pkg-config libgtk-4-dev libadwaita-1-dev
```

  Rust 1.92 o superior (Ubuntu 26.10 trae 1.97).

Si `msi_ec` no reconoce tu firmware, puede que necesites forzar uno compatible, por ejemplo:

```sh
echo 'options msi_ec firmware=15B1EMS1.103' | sudo tee /etc/modprobe.d/msi_ec.conf
```

## Instalación

```sh
git clone https://github.com/acid0ikario/termica.git
cd termica
./install.sh
```

La primera compilación tarda un par de minutos. Después **cierra sesión y vuelve a entrar** para que GNOME cargue la extensión (en Wayland no se pueden
recargar extensiones sin reiniciar la sesión).

El instalador:

1. Compila en modo release e instala el servicio en `/usr/local/libexec/termicad`; lo registra en systemd y lo arranca.
2. Instala la política de D-Bus y la acción de polkit `com.ikario.termica.manage`. Un usuario con
   sesión activa puede cambiar perfiles **sin contraseña**.
3. Instala la app en `/usr/local/bin/termica` con su icono y su acceso directo.
4. Copia la extensión a `~/.local/share/gnome-shell/extensions/termica@ikario` y la activa.
5. Si existe un servicio anterior llamado `msi-fan-curve`, lo desactiva (Térmica lo reemplaza), y borra
   los restos de la versión 0.1 en Python.

Para actualizar: `git pull && ./install.sh`.

Los perfiles se guardan en `/etc/termica/profiles.json`. El perfil activo se mantiene al reiniciar.

## Uso

- **Cambiar de perfil:** menú del icono de la barra → *Perfiles*, o botón **Activar** en la app.
- **Editar perfiles:** app Térmica. Los cambios en el perfil activo se aplican al guardar.
- **Cooler Boost / Turbo Boost rápidos:** interruptores del menú del icono. Duran hasta el siguiente cambio
  de perfil; para que sean permanentes, guárdalos en el perfil.
- **Elegir qué datos ver en la barra:** app → botón del termómetro, menú del icono → *Mostrar en la barra*,
  o Extensiones → Térmica → ⚙.
- **Ocultar el icono:** menú del icono → *Salir de Térmica*. El perfil sigue aplicado. Para volver a
  mostrarlo: app → botón del termómetro → *Mostrar icono en la barra*.

### Seguridad de los valores

El servicio rechaza perfiles peligrosos: curvas donde el ventilador baja al subir la temperatura, un
último tramo por debajo de 70, temperaturas fuera de orden, PL2 menor que PL1, o valores fuera de rango.

## Desinstalar

```sh
./uninstall.sh
```

Quita el servicio, la app y la extensión, y vuelve a activar `msi-fan-curve` si existía. Tus perfiles se
conservan en `/etc/termica/profiles.json`.

## Desarrollo

Puedes probar todo sin tocar el hardware ni instalar nada: el servicio tiene un modo `--dev` que usa el
bus de sesión, no pide permisos y solo registra lo que aplicaría.

```sh
mkdir -p /tmp/termica-dev
TERMICA_CONF_DIR=/tmp/termica-dev cargo run -p termicad -- --dev &
TERMICA_DEV=1 cargo run -p termica
```

Pruebas y análisis: `cargo test` y `cargo clippy --all-targets`.

API de D-Bus (`com.ikario.Termica1` en `/com/ikario/Termica1`):

| Método | Descripción |
|---|---|
| `GetProfiles() → s` | JSON con los perfiles y el activo |
| `GetStatus() → s` | Estado completo (incluye datos de `nvidia-smi` si la GPU está despierta) |
| `GetTemps() → s` | Estado ligero solo desde sysfs, sin despertar la GPU |
| `Activate(s id) → s` | Activa un perfil; devuelve avisos si alguna parte falló |
| `SaveProfile(s json) → s` | Crea o reemplaza un perfil |
| `DeleteProfile(s id)` | Borra un perfil (no el activo) |
| `SetQuick(s clave, b valor)` | Cambio rápido de `cooler_boost` o `turbo` |

Señales: `ActiveChanged(s)`, `ProfilesChanged()`, `QuickChanged(s, b)`.

```
crates/termica-core/  tipos de perfil y validación, compartidos por el servicio y la app
crates/termicad/      servicio (zbus, tokio, nvml-wrapper)
crates/termica/       app (gtk4-rs, libadwaita-rs)
extension/            extensión de GNOME Shell (GJS: GNOME solo admite JavaScript)
data/                 systemd, D-Bus, polkit, acceso directo, icono y perfiles iniciales
```

La versión 0.1 estaba escrita en Python; está en el historial de git (etiqueta `v0.1-python`).

## Licencia

[GPL-3.0](LICENSE)
