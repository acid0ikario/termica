import {tr} from './i18n.js';
// Datos que se pueden mostrar en la barra superior.
// light: se obtiene sin despertar la GPU. short: nombre corto opcional.
// pair: comparte icono con otro dato, así que siempre lleva su nombre corto.
export const METRICS = [
    {id: 'profile', get title() { return tr("Active profile"); }, icon: 'profile', short: '', light: true, sample: tr("Quiet")},
    {id: 'cpu-temp', get title() { return tr("CPU temperature"); }, icon: 'cpu', short: 'CPU', light: true, temp: true, sample: '64°'},
    {id: 'gpu-temp', get title() { return tr("GPU temperature"); }, icon: 'gpu', short: 'GPU', light: true, temp: true, sample: '58°'},
    {id: 'cpu-power', get title() { return tr("CPU power"); }, icon: 'bolt', short: 'CPU', pair: true, light: true, sample: '12 W'},
    {id: 'gpu-power', get title() { return tr("GPU power"); }, icon: 'bolt', short: 'GPU', pair: true, sample: '21 W'},
    {id: 'cpu-fan-rpm', get title() { return tr("Fan 1 (CPU), RPM"); }, icon: 'fan', short: '1', pair: true, light: true, sample: '2980'},
    {id: 'gpu-fan-rpm', get title() { return tr("Fan 2 (GPU), RPM"); }, icon: 'fan', short: '2', pair: true, light: true, sample: '3000'},
    {id: 'cpu-fan-pct', get title() { return tr("Fan 1 (CPU), %"); }, icon: 'fan', short: '1', pair: true, light: true, sample: '35%'},
    {id: 'gpu-fan-pct', get title() { return tr("Fan 2 (GPU), %"); }, icon: 'fan', short: '2', pair: true, light: true, sample: '50%'},
    {id: 'gpu-clock', get title() { return tr("GPU core clock"); }, icon: 'speed', short: 'GPU', sample: '795 MHz'},
    {id: 'gpu-mem-clock', get title() { return tr("GPU memory clock"); }, icon: 'wave', short: 'MEM', sample: '5501 MHz'},
    {id: 'gpu-util', get title() { return tr("GPU utilization"); }, icon: 'activity', short: 'GPU', sample: '12%'},
    {id: 'vram', get title() { return tr("GPU memory used (VRAM)"); }, icon: 'vram', short: 'VRAM', sample: '1.2 GB'},
    {id: 'ram', get title() { return tr("RAM used"); }, icon: 'ram', short: 'RAM', light: true, sample: '7.4 GB'},
];

export const DEFAULT_ITEMS = ['cpu-temp', 'gpu-temp'];

const NV_OFF = '—';

// Devuelve el texto de un dato a partir del estado del servicio
export function formatMetric(id, s) {
    const v = (x, f) => (x === null || x === undefined ? NV_OFF : f(x));
    const nv = (x, f) => (s.gpu_awake ? v(x, f) : NV_OFF);
    switch (id) {
    case 'profile': return s.profile_name ?? NV_OFF;
    case 'cpu-temp': return v(s.cpu_temp, x => `${x}°`);
    case 'gpu-temp': return v(s.gpu_temp, x => `${x}°`);
    case 'cpu-power': return v(s.cpu_power, x => `${Math.round(x)} W`);
    case 'gpu-power': return nv(s.gpu_power, x => `${x} W`);
    case 'cpu-fan-rpm': return v(s.cpu_rpm, x => `${x}`);
    case 'gpu-fan-rpm': return v(s.gpu_rpm, x => `${x}`);
    case 'cpu-fan-pct': return v(s.cpu_fan, x => `${x}%`);
    case 'gpu-fan-pct': return v(s.gpu_fan, x => `${x}%`);
    case 'gpu-clock': return nv(s.gpu_clock, x => `${x} MHz`);
    case 'gpu-mem-clock': return nv(s.gpu_mem_clock, x => `${x} MHz`);
    case 'gpu-util': return nv(s.gpu_util, x => `${x}%`);
    case 'vram': return nv(s.vram_used, x => `${(x / 1024).toFixed(1)} GB`);
    case 'ram': return v(s.ram_used, x => `${x.toFixed(1)} GB`);
    }
    return NV_OFF;
}

export function metricTemp(id, s) {
    if (id === 'cpu-temp')
        return s.cpu_temp ?? null;
    if (id === 'gpu-temp')
        return s.gpu_temp ?? null;
    return null;
}
