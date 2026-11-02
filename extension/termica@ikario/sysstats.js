// System usage read straight from /proc, like the Resources app: no root, no service,
// no libgtop. CPU usage and network speeds are rates, so they need two samples.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const GIB_KB = 1024 * 1024;

function readText(path) {
    try {
        const [ok, bytes] = GLib.file_get_contents(path);
        return ok ? new TextDecoder().decode(bytes) : null;
    } catch {
        return null;
    }
}

// Aggregate "cpu" line of /proc/stat → {busy, total} jiffies
export function parseCpu(text) {
    const line = text?.split('\n').find(l => l.startsWith('cpu '));
    if (!line)
        return null;
    // user nice system idle iowait irq softirq steal (guest time is already counted in user)
    const v = line.trim().split(/\s+/).slice(1, 9).map(Number);
    const total = v.reduce((a, b) => a + b, 0);
    const idle = v[3] + (v[4] ?? 0);
    return {busy: total - idle, total};
}

// /proc/meminfo → used RAM and swap in GiB (swap is null when there is no swap)
export function parseMem(text) {
    const kb = {};
    for (const m of text?.matchAll(/^(\w+):\s+(\d+) kB$/gm) ?? [])
        kb[m[1]] = Number(m[2]);
    if (!kb.MemTotal)
        return null;
    const avail = kb.MemAvailable ?? kb.MemFree ?? 0;
    return {
        ram: (kb.MemTotal - avail) / GIB_KB,
        swap: kb.SwapTotal ? (kb.SwapTotal - (kb.SwapFree ?? 0)) / GIB_KB : null,
    };
}

// /proc/net/dev → {rx, tx} bytes summed over the given interfaces
export function parseNet(text, ifaces) {
    let rx = 0, tx = 0;
    for (const line of text?.split('\n').slice(2) ?? []) {
        const [name, rest] = line.split(':');
        if (!rest || !ifaces.has(name.trim()))
            continue;
        const v = rest.trim().split(/\s+/).map(Number);
        rx += v[0];
        tx += v[8];
    }
    return {rx, tx};
}

// Hardware interfaces only (Wi‑Fi, Ethernet, USB tethering). Loopback, bridges, VPN
// tunnels and container links have no device, which also avoids counting traffic twice.
function physicalInterfaces() {
    const set = new Set();
    try {
        const en = Gio.File.new_for_path('/sys/class/net').enumerate_children('standard::name',
            Gio.FileQueryInfoFlags.NONE, null);
        for (let info; (info = en.next_file(null));) {
            const name = info.get_name();
            if (GLib.file_test(`/sys/class/net/${name}/device`, GLib.FileTest.EXISTS))
                set.add(name);
        }
        en.close(null);
    } catch {}
    return set;
}

// Short rate text: "0 kB/s", "640 kB/s", "2.4 MB/s"
export function formatRate(bytesPerSec) {
    if (bytesPerSec >= 1e9)
        return `${(bytesPerSec / 1e9).toFixed(1)} GB/s`;
    if (bytesPerSec >= 999.5e3)
        return `${(bytesPerSec / 1e6).toFixed(1)} MB/s`;
    return `${Math.round(bytesPerSec / 1e3)} kB/s`;
}

export class SystemStats {
    constructor() {
        this._cpu = null;
        this._net = null;
        this._ifaces = null;
        this._ifacesAt = 0;
    }

    // Fields merged into the service status: cpu_usage (%), ram_used and swap_used (GiB),
    // net_down and net_up (bytes/s). Rates are null on the first call.
    sample() {
        const out = {cpu_usage: null, net_down: null, net_up: null};
        const now = GLib.get_monotonic_time();

        const cpu = parseCpu(readText('/proc/stat'));
        if (cpu && this._cpu && cpu.total > this._cpu.total)
            out.cpu_usage = Math.round(100 * (cpu.busy - this._cpu.busy) / (cpu.total - this._cpu.total));
        this._cpu = cpu;

        const mem = parseMem(readText('/proc/meminfo'));
        if (mem) {
            out.ram_used = Math.round(mem.ram * 10) / 10;
            out.swap_used = mem.swap === null ? null : Math.round(mem.swap * 10) / 10;
        }

        // interfaces come and go (USB tethering, docks): look again every 30 s
        if (!this._ifaces || now - this._ifacesAt > 30_000_000) {
            const ifaces = physicalInterfaces();
            if ([...ifaces].sort().join() !== [...this._ifaces ?? []].sort().join())
                this._net = null;
            this._ifaces = ifaces;
            this._ifacesAt = now;
        }
        const net = parseNet(readText('/proc/net/dev'), this._ifaces);
        if (this._net) {
            const secs = (now - this._net.at) / 1e6;
            // counters can go backwards when an interface is reset
            if (secs > 0) {
                out.net_down = Math.max(0, net.rx - this._net.rx) / secs;
                out.net_up = Math.max(0, net.tx - this._net.tx) / secs;
            }
        }
        this._net = {...net, at: now};
        return out;
    }
}
