#!/usr/bin/python3
"""termicad: aplica perfiles térmicos (GPU NVIDIA, CPU Intel, ventiladores MSI).

Corre como root y expone una API en el bus de sistema (com.ikario.Termica1).
Los cambios pasan por polkit (acción com.ikario.termica.manage).
"""
import json
import logging
import os
import re
import subprocess
import tempfile
import time
import unicodedata

import dbus
import dbus.exceptions
import dbus.mainloop.glib
import dbus.service
from gi.repository import GLib

BUS_NAME = IFACE = 'com.ikario.Termica1'
OBJ_PATH = '/com/ikario/Termica1'
POLKIT_ACTION = 'com.ikario.termica.manage'

# --dev: bus de sesión, sin polkit y sin tocar el hardware (para probar la app)
DEV = '--dev' in __import__('sys').argv
CONF_DIR = os.environ.get('TERMICA_CONF_DIR', '/etc/termica')
PROFILES_FILE = os.path.join(CONF_DIR, 'profiles.json')
_HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULTS_FILE = next((p for p in (os.path.join(_HERE, 'default-profiles.json'),
                                  os.path.join(_HERE, '..', 'data', 'default-profiles.json'))
                      if os.path.exists(p)), os.path.join(_HERE, 'default-profiles.json'))

EC_IO = '/sys/kernel/debug/ec/ec0/io'
MSI = '/sys/devices/platform/msi-ec'
RAPL = '/sys/class/powercap/intel-rapl:0'
GPU_PCI = '/sys/bus/pci/devices/0000:01:00.0'
NO_TURBO = '/sys/devices/system/cpu/intel_pstate/no_turbo'

# Direcciones del EC del MSI Stealth 15M (firmware 15B1EMS1). En otros modelos pueden ser
# distintas: escribir ahí podría romper el control térmico, así que se verifica el firmware.
SUPPORTED_EC_FW = ('15B1EMS1',)
ALLOW_UNKNOWN_EC = os.path.join(CONF_DIR, 'allow-unknown-ec')
EC_CURVES = {
    'cpu': (0x6A, 0x72),  # (temperaturas x6, velocidades x7)
    'gpu': (0x82, 0x8A),
}
# Curva que trae el equipo al arrancar (leída antes de cualquier cambio)
FACTORY_CURVES = {
    'cpu': {'temps': [68, 78, 85, 90, 94, 97], 'speeds': [0, 30, 38, 55, 75, 100, 120]},
    'gpu': {'temps': [60, 70, 76, 82, 86, 90], 'speeds': [0, 30, 38, 50, 65, 85, 110]},
}
FAN_MODES = ('auto', 'advanced')
SHIFT_MODES = ('eco', 'comfort', 'turbo')
GPU_CORE_MIN, GPU_CORE_MAX = 210, 2100
GPU_MEM_MIN = 405
GPU_MEM_STEPS = (810, 5501, 6001)
SPEED_MAX = 150        # el EC usa una escala que llega a 150
LAST_SPEED_MIN = 70    # seguridad: el último tramo nunca por debajo de esto

log = logging.getLogger('termicad')


# ---------------------------------------------------------------- utilidades

def read(path, default=None):
    try:
        with open(path) as f:
            return f.read().strip()
    except OSError:
        return default


def read_int(path, default=None):
    v = read(path)
    try:
        return int(v)
    except (TypeError, ValueError):
        return default


def write_sysfs(path, value):
    with open(path, 'w') as f:
        f.write(str(value))


def find_hwmon(name=None, file=None):
    base = '/sys/class/hwmon'
    for h in sorted(os.listdir(base)):
        p = os.path.join(base, h)
        if name and read(os.path.join(p, 'name')) != name:
            continue
        if file and not os.path.exists(os.path.join(p, file)):
            continue
        return p
    return None


def slugify(text):
    text = unicodedata.normalize('NFKD', text).encode('ascii', 'ignore').decode()
    return re.sub(r'[^a-z0-9]+', '-', text.lower()).strip('-') or 'perfil'


def atomic_write_json(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix='.tmp-')
    with os.fdopen(fd, 'w') as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write('\n')
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


# ---------------------------------------------------------------- validación

def _int(v, lo, hi, what):
    if isinstance(v, bool) or not isinstance(v, (int, float)) or v != int(v):
        raise ValueError(f'{what}: debe ser un número entero')
    v = int(v)
    if not lo <= v <= hi:
        raise ValueError(f'{what}: debe estar entre {lo} y {hi}')
    return v


def validate_curve(c, what):
    if not isinstance(c, dict):
        raise ValueError(f'{what}: curva inválida')
    temps = [_int(t, 30, 100, f'{what}, temperatura') for t in c.get('temps', [])]
    speeds = [_int(s, 0, SPEED_MAX, f'{what}, velocidad') for s in c.get('speeds', [])]
    if len(temps) != 6 or len(speeds) != 7:
        raise ValueError(f'{what}: se esperan 6 temperaturas y 7 velocidades')
    if any(b <= a for a, b in zip(temps, temps[1:])):
        raise ValueError(f'{what}: las temperaturas deben ir en aumento')
    if any(b < a for a, b in zip(speeds, speeds[1:])):
        raise ValueError(f'{what}: la velocidad no puede bajar cuando sube la temperatura')
    if speeds[-1] < LAST_SPEED_MIN:
        raise ValueError(f'{what}: por seguridad el último punto debe ser al menos {LAST_SPEED_MIN}')
    return {'temps': temps, 'speeds': speeds}


def validate_profile(p):
    if not isinstance(p, dict):
        raise ValueError('Perfil inválido')
    name = str(p.get('name', '')).strip()[:40]
    if not name:
        raise ValueError('El perfil necesita un nombre')

    gpu = p.get('gpu') or {}
    core = gpu.get('core_max')
    core = None if core is None else _int(core, 300, GPU_CORE_MAX, 'GPU, núcleo máximo')
    mem = gpu.get('mem_max')
    if mem is not None and mem not in GPU_MEM_STEPS:
        raise ValueError(f'GPU, memoria máxima: valores posibles {GPU_MEM_STEPS}')

    cpu = p.get('cpu') or {}
    pl1 = _int(cpu.get('pl1'), 5, 115, 'CPU, potencia sostenida (PL1)')
    pl2 = _int(cpu.get('pl2'), 5, 200, 'CPU, potencia de ráfaga (PL2)')
    if pl2 < pl1:
        raise ValueError('CPU: la potencia de ráfaga (PL2) no puede ser menor que la sostenida (PL1)')

    fans = p.get('fans') or {}
    mode = fans.get('mode')
    if mode not in FAN_MODES:
        raise ValueError(f'Ventiladores: modo debe ser uno de {FAN_MODES}')
    curves = fans.get('curves') or {}
    shift = p.get('shift_mode')
    if shift is not None and shift not in SHIFT_MODES:
        raise ValueError(f'Modo MSI: debe ser uno de {SHIFT_MODES}')

    return {
        'id': str(p.get('id') or ''),
        'name': name,
        'gpu': {'core_max': core, 'mem_max': mem},
        'cpu': {'pl1': pl1, 'pl2': pl2, 'turbo': bool(cpu.get('turbo', True))},
        'fans': {
            'mode': mode,
            'cooler_boost': bool(fans.get('cooler_boost', False)),
            'curves': {
                'cpu': validate_curve(curves.get('cpu', FACTORY_CURVES['cpu']), 'Curva CPU'),
                'gpu': validate_curve(curves.get('gpu', FACTORY_CURVES['gpu']), 'Curva GPU'),
            },
        },
        'shift_mode': shift,
    }


# ---------------------------------------------------------------- hardware

def ensure_ec():
    if not os.path.exists(EC_IO) or read('/sys/module/ec_sys/parameters/write_support') != 'Y':
        subprocess.run(['modprobe', '-r', 'ec_sys'], capture_output=True)
        subprocess.run(['modprobe', 'ec_sys', 'write_support=1'], capture_output=True)
    if not os.path.isdir(MSI):
        subprocess.run(['modprobe', 'msi_ec'], capture_output=True)


def ec_supported():
    fw = read(f'{MSI}/fw_version', '')
    return fw.startswith(SUPPORTED_EC_FW) or os.path.exists(ALLOW_UNKNOWN_EC)


def ec_write(addr, values):
    with open(EC_IO, 'r+b', buffering=0) as f:
        f.seek(addr)
        f.write(bytes(values))


def nvsmi(*args):
    r = subprocess.run(['nvidia-smi', *args], capture_output=True, text=True, timeout=20)
    if r.returncode != 0:
        raise RuntimeError((r.stdout + r.stderr).strip().splitlines()[-1] if (r.stdout + r.stderr).strip() else f'nvidia-smi {args} falló')


def apply_fans(p):
    ensure_ec()
    fans = p['fans']
    if ec_supported():
        curves = fans['curves'] if fans['mode'] == 'advanced' else FACTORY_CURVES
        for key, (t_addr, s_addr) in EC_CURVES.items():
            ec_write(t_addr, curves[key]['temps'])
            ec_write(s_addr, curves[key]['speeds'])
    elif fans['mode'] == 'advanced':
        raise RuntimeError(f'firmware {read(f"{MSI}/fw_version")} no verificado: no se escribe la curva '
                           f'(ver README, {ALLOW_UNKNOWN_EC})')
    write_sysfs(f'{MSI}/fan_mode', fans['mode'])
    write_sysfs(f'{MSI}/cooler_boost', 'on' if fans['cooler_boost'] else 'off')
    if p.get('shift_mode'):
        write_sysfs(f'{MSI}/shift_mode', p['shift_mode'])


def apply_cpu(p):
    # PL2 primero cuando baja, PL1 primero cuando sube, para no dejar PL2 < PL1
    pl1, pl2 = p['cpu']['pl1'] * 1_000_000, p['cpu']['pl2'] * 1_000_000
    write_sysfs(f'{RAPL}/constraint_1_power_limit_uw', max(pl2, read_int(f'{RAPL}/constraint_0_power_limit_uw', 0)))
    write_sysfs(f'{RAPL}/constraint_0_power_limit_uw', pl1)
    write_sysfs(f'{RAPL}/constraint_1_power_limit_uw', pl2)
    set_turbo(p['cpu']['turbo'])


def set_turbo(on):
    write_sysfs(NO_TURBO, 0 if on else 1)


def set_cooler_boost(on):
    write_sysfs(f'{MSI}/cooler_boost', 'on' if on else 'off')


def apply_gpu(p):
    core, mem = p['gpu']['core_max'], p['gpu']['mem_max']
    nvsmi('-lgc', f'{GPU_CORE_MIN},{core}') if core else nvsmi('-rgc')
    nvsmi('-lmc', f'{GPU_MEM_MIN},{mem}') if mem else nvsmi('-rmc')


def apply_profile(p, parts=('fans', 'cpu', 'gpu')):
    """Aplica el perfil. Devuelve una lista de avisos (vacía si todo fue bien)."""
    if DEV:
        log.info('[dev] aplicaría "%s" (%s): %s', p['name'], ', '.join(parts), json.dumps(p, ensure_ascii=False))
        return []
    steps = {'fans': ('ventiladores', apply_fans), 'cpu': ('procesador', apply_cpu), 'gpu': ('gráfica', apply_gpu)}
    warnings = []
    for part in parts:
        label, fn = steps[part]
        try:
            fn(p)
        except Exception as e:  # seguimos con las demás partes
            log.exception('Error aplicando %s', label)
            warnings.append(f'{label}: {e}')
    log.info('Perfil "%s" aplicado (%s)%s', p['name'], ', '.join(parts),
             f' con avisos: {warnings}' if warnings else '')
    return warnings


def gpu_awake():
    return read(f'{GPU_PCI}/power/runtime_status', 'active') != 'suspended'


# ---------------------------------------------------------------- servicio

class Service(dbus.service.Object):
    def __init__(self, bus):
        super().__init__(bus, OBJ_PATH)
        self.bus = bus
        self.data = self._load()
        self._nv_cache = (0.0, None)
        self._gpu_was_awake = gpu_awake()
        self.overrides = {}   # cambios rápidos desde el panel, hasta cambiar de perfil
        self._energy = None   # (instante, energy_uj) para calcular el consumo del CPU
        self._cpu_watts = None

        if not DEV:
            bus.add_signal_receiver(self._on_prepare_for_sleep, 'PrepareForSleep',
                                    'org.freedesktop.login1.Manager', 'org.freedesktop.login1')
        GLib.timeout_add_seconds(5, self._watch_gpu)
        GLib.idle_add(self._apply_active)

    # --- datos
    def _load(self):
        for path in (PROFILES_FILE, DEFAULTS_FILE):
            try:
                with open(path) as f:
                    raw = json.load(f)
                profiles = []
                for p in raw['profiles']:
                    v = validate_profile(p)
                    v['id'] = p['id']
                    profiles.append(v)
                active = raw.get('active')
                if active not in {p['id'] for p in profiles}:
                    active = profiles[0]['id']
                data = {'active': active, 'profiles': profiles}
                if path != PROFILES_FILE:
                    atomic_write_json(PROFILES_FILE, data)
                return data
            except FileNotFoundError:
                continue
            except Exception:
                log.exception('No se pudo leer %s', path)
        raise SystemExit('No hay perfiles válidos')

    def _save(self):
        atomic_write_json(PROFILES_FILE, self.data)

    def _get(self, pid):
        for p in self.data['profiles']:
            if p['id'] == pid:
                return p
        raise ValueError('Ese perfil no existe')

    def _active(self):
        return self._get(self.data['active'])

    def _apply_active(self, parts=('fans', 'cpu', 'gpu')):
        apply_profile(self._active(), parts)
        self._apply_overrides()
        return False

    def _apply_overrides(self):
        if DEV:
            return
        for key, value in self.overrides.items():
            try:
                {'cooler_boost': set_cooler_boost, 'turbo': set_turbo}[key](value)
            except Exception:
                log.exception('No se pudo aplicar %s', key)

    # --- eventos del sistema
    def _on_prepare_for_sleep(self, going_to_sleep):
        if not going_to_sleep:
            log.info('Regreso de suspensión: reaplico el perfil')
            GLib.timeout_add_seconds(4, self._apply_active)

    def _watch_gpu(self):
        awake = gpu_awake()
        if awake and not self._gpu_was_awake:
            log.info('La GPU despertó: reaplico sus límites')
            self._apply_active(('gpu',))
        self._gpu_was_awake = awake
        return True

    # --- polkit
    def _authorized(self, sender, ok, err, fn):
        if DEV:
            try:
                ok(fn())
            except Exception as e:
                err(dbus.exceptions.DBusException(str(e), name=IFACE + '.Error.Invalid'))
            return
        authority = self.bus.get_object('org.freedesktop.PolicyKit1', '/org/freedesktop/PolicyKit1/Authority')
        subject = dbus.Struct((dbus.String('system-bus-name'),
                               dbus.Dictionary({'name': dbus.String(sender)}, signature='sv')),
                              signature='sa{sv}')

        def on_reply(result):
            if not result[0]:
                err(dbus.exceptions.DBusException('No tienes permiso para cambiar perfiles',
                                                  name=IFACE + '.Error.NotAuthorized'))
                return
            try:
                ok(fn())
            except ValueError as e:
                err(dbus.exceptions.DBusException(str(e), name=IFACE + '.Error.Invalid'))
            except Exception as e:
                log.exception('Error')
                err(dbus.exceptions.DBusException(str(e), name=IFACE + '.Error.Failed'))

        authority.CheckAuthorization(subject, POLKIT_ACTION, dbus.Dictionary({}, signature='ss'),
                                     dbus.UInt32(1), '',
                                     dbus_interface='org.freedesktop.PolicyKit1.Authority',
                                     reply_handler=on_reply, error_handler=err, timeout=300)

    # --- API de lectura
    @dbus.service.method(IFACE, out_signature='s')
    def GetProfiles(self):
        return json.dumps(self.data, ensure_ascii=False)

    def _cpu_power(self):
        now, e = time.monotonic(), read_int(f'{RAPL}/energy_uj')
        last, self._energy = self._energy, (now, e)
        if last and e is not None and last[1] is not None and e >= last[1] and now - last[0] > 0.3:
            self._cpu_watts = round((e - last[1]) / 1e6 / (now - last[0]), 1)
        return self._cpu_watts

    def _light(self):
        """Datos que salen de sysfs: baratos y sin despertar la GPU."""
        s = {}
        core = find_hwmon('coretemp')
        pkg = read_int(f'{core}/temp1_input') if core else None
        s['cpu_temp'] = round(pkg / 1000) if pkg else read_int(f'{MSI}/cpu/realtime_temperature')
        s['gpu_temp'] = read_int(f'{MSI}/gpu/realtime_temperature')
        s['cpu_fan'] = read_int(f'{MSI}/cpu/realtime_fan_speed')
        s['gpu_fan'] = read_int(f'{MSI}/gpu/realtime_fan_speed')
        fan = find_hwmon(file='fan1_input')
        s['cpu_rpm'] = read_int(f'{fan}/fan1_input') if fan else None
        s['gpu_rpm'] = read_int(f'{fan}/fan2_input') if fan else None
        s['turbo'] = read(NO_TURBO) == '0'
        s['cpu_power'] = self._cpu_power()
        mem = {}
        for line in (read('/proc/meminfo') or '').splitlines():
            k, _, v = line.partition(':')
            mem[k] = int(v.split()[0]) if v.strip() else 0
        if mem.get('MemTotal'):
            s['ram_used'] = round((mem['MemTotal'] - mem.get('MemAvailable', 0)) / 1048576, 1)
            s['ram_total'] = round(mem['MemTotal'] / 1048576, 1)
        s['gpu_awake'] = gpu_awake()
        s['active'] = self.data['active']
        s['profile_name'] = self._active()['name']
        return s

    @dbus.service.method(IFACE, out_signature='s')
    def GetStatus(self):
        s = self._light()
        s['fan_mode'] = read(f'{MSI}/fan_mode')
        s['cooler_boost'] = read(f'{MSI}/cooler_boost') == 'on'
        s['shift_mode'] = read(f'{MSI}/shift_mode')
        s['pl1'] = (read_int(f'{RAPL}/constraint_0_power_limit_uw') or 0) // 1_000_000
        s['pl2'] = (read_int(f'{RAPL}/constraint_1_power_limit_uw') or 0) // 1_000_000
        if s['gpu_awake']:
            nv = self._nvidia()
            if nv:
                s.update(nv)
        s['overrides'] = self.overrides
        return json.dumps(s)

    @dbus.service.method(IFACE, out_signature='s')
    def GetTemps(self):
        """Versión ligera para la barra superior: solo sysfs, sin despertar la GPU."""
        return json.dumps(self._light(), ensure_ascii=False)

    def _nvidia(self):
        # nvidia-smi tarda; se cachea para no saturar si varios clientes preguntan
        stamp, cached = self._nv_cache
        if time.monotonic() - stamp < 1.5:
            return cached
        try:
            out = subprocess.run(['nvidia-smi', '--query-gpu=temperature.gpu,clocks.gr,clocks.mem,power.draw,pstate,'
                                  'memory.used,memory.total,utilization.gpu',
                                  '--format=csv,noheader,nounits'],
                                 capture_output=True, text=True, timeout=5).stdout.strip()
            t, gr, mem, pw, ps, vu, vt, util = [x.strip() for x in out.split(',')]
            cached = {'gpu_temp': int(t), 'gpu_clock': int(gr), 'gpu_mem_clock': int(mem),
                      'gpu_power': round(float(pw)) if pw not in ('[N/A]', 'N/A') else None, 'gpu_pstate': ps,
                      'vram_used': int(vu), 'vram_total': int(vt),
                      'gpu_util': int(util) if util.isdigit() else None}
        except Exception:
            cached = None
        self._nv_cache = (time.monotonic(), cached)
        return cached

    # --- API de escritura
    @dbus.service.method(IFACE, in_signature='s', out_signature='s', sender_keyword='sender',
                         async_callbacks=('ok', 'err'))
    def Activate(self, pid, sender=None, ok=None, err=None):
        def do():
            p = self._get(str(pid))
            self.data['active'] = p['id']
            self._save()
            self.overrides.clear()
            warnings = apply_profile(p)
            self.ActiveChanged(p['id'])
            return '\n'.join(warnings)
        self._authorized(sender, ok, err, do)

    @dbus.service.method(IFACE, in_signature='s', out_signature='s', sender_keyword='sender',
                         async_callbacks=('ok', 'err'))
    def SaveProfile(self, raw, sender=None, ok=None, err=None):
        def do():
            try:
                incoming = json.loads(raw)
            except json.JSONDecodeError:
                raise ValueError('JSON inválido')
            p = validate_profile(incoming)
            ids = [x['id'] for x in self.data['profiles']]
            if p['id'] in ids:
                self.data['profiles'][ids.index(p['id'])] = p
            else:
                base = slugify(p['name'])
                pid, n = base, 2
                while pid in ids:
                    pid, n = f'{base}-{n}', n + 1
                p['id'] = pid
                self.data['profiles'].append(p)
            self._save()
            warnings = apply_profile(p) if p['id'] == self.data['active'] else []
            self.ProfilesChanged()
            return json.dumps({'id': p['id'], 'warnings': warnings}, ensure_ascii=False)
        self._authorized(sender, ok, err, do)

    @dbus.service.method(IFACE, in_signature='s', out_signature='', sender_keyword='sender',
                         async_callbacks=('ok', 'err'))
    def DeleteProfile(self, pid, sender=None, ok=None, err=None):
        def do():
            p = self._get(str(pid))
            if p['id'] == self.data['active']:
                raise ValueError('No se puede borrar el perfil activo')
            if len(self.data['profiles']) <= 1:
                raise ValueError('Debe quedar al menos un perfil')
            self.data['profiles'].remove(p)
            self._save()
            self.ProfilesChanged()
        self._authorized(sender, lambda _r: ok(), err, do)

    @dbus.service.method(IFACE, in_signature='sb', out_signature='', sender_keyword='sender',
                         async_callbacks=('ok', 'err'))
    def SetQuick(self, key, value, sender=None, ok=None, err=None):
        """Cambio rápido (cooler_boost o turbo) que dura hasta el próximo cambio de perfil."""
        def do():
            if key not in ('cooler_boost', 'turbo'):
                raise ValueError('Ajuste desconocido')
            self.overrides[str(key)] = bool(value)
            if DEV:
                log.info('[dev] %s = %s', key, bool(value))
            else:
                {'cooler_boost': set_cooler_boost, 'turbo': set_turbo}[key](bool(value))
            self.QuickChanged(str(key), bool(value))
        self._authorized(sender, lambda _r: ok(), err, do)

    @dbus.service.signal(IFACE, signature='sb')
    def QuickChanged(self, key, value):
        pass

    @dbus.service.signal(IFACE, signature='s')
    def ActiveChanged(self, pid):
        pass

    @dbus.service.signal(IFACE, signature='')
    def ProfilesChanged(self):
        pass


def main():
    logging.basicConfig(level=logging.INFO, format='%(levelname)s %(message)s')
    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus() if DEV else dbus.SystemBus()
    name = dbus.service.BusName(BUS_NAME, bus, do_not_queue=True)  # noqa: F841 (mantener vivo)
    Service(bus)
    log.info('termicad listo')
    GLib.MainLoop().run()


if __name__ == '__main__':
    main()
