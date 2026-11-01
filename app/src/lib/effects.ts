// Descripción de los efectos para la interfaz: nombres, valores por defecto y
// controles. Los rangos coinciden con `EffectParams::sanitized` (crates/mn-fx).
import type { EffectParams, EffectSlot, EffectType } from './api'

export const EFFECT_LABELS: Record<EffectType, string> = {
  eq: 'Ecualizador',
  compressor: 'Compresor',
  reverb: 'Reverb',
  delay: 'Delay',
  limiter: 'Limitador',
}

export const EFFECT_ORDER: EffectType[] = ['eq', 'compressor', 'delay', 'reverb', 'limiter']

export function defaultParams(type: EffectType): EffectParams {
  switch (type) {
    case 'eq':
      return {
        type,
        highpass: 0,
        low_shelf: { freq: 120, gain_db: 0, q: 0.707 },
        mid1: { freq: 500, gain_db: 0, q: 1 },
        mid2: { freq: 2500, gain_db: 0, q: 1 },
        high_shelf: { freq: 8000, gain_db: 0, q: 0.707 },
        lowpass: 0,
      }
    case 'compressor':
      return { type, threshold_db: -18, ratio: 3, attack_ms: 10, release_ms: 120, knee_db: 6, makeup_db: 0 }
    case 'reverb':
      return { type, room_size: 0.6, damping: 0.5, width: 1, predelay_ms: 20, mix: 0.25 }
    case 'delay':
      return { type, time_ms: 375, feedback: 0.35, high_cut: 6000, ping_pong: true, mix: 0.25 }
    case 'limiter':
      return { type, input_db: 0, ceiling_db: -1, release_ms: 80, true_peak: false }
  }
}

export function newSlot(type: EffectType): EffectSlot {
  return { id: crypto.randomUUID(), enabled: true, params: defaultParams(type) }
}

/** Un control deslizante. `path` admite un nivel de anidación ("mid1.gain_db"). */
export interface Control {
  path: string
  label: string
  min: number
  max: number
  step: number
  unit: string
  /** Escala logarítmica (frecuencias y tiempos). */
  log?: boolean
  /** 0 significa «desactivado» (paso alto / paso bajo). */
  offAtZero?: boolean
  /** Se muestra como porcentaje (valores 0–1). */
  percent?: boolean
}

const band = (name: string, label: string, lo: number, hi: number, withQ = true): Control[] => [
  { path: `${name}.freq`, label: `${label} · frec.`, min: lo, max: hi, step: 1, unit: 'Hz', log: true },
  { path: `${name}.gain_db`, label: `${label} · ganancia`, min: -18, max: 18, step: 0.5, unit: 'dB' },
  ...(withQ ? [{ path: `${name}.q`, label: `${label} · Q`, min: 0.1, max: 10, step: 0.01, unit: '', log: true }] : []),
]

export const CONTROLS: Record<EffectType, Control[]> = {
  eq: [
    { path: 'highpass', label: 'Paso alto', min: 20, max: 1000, step: 1, unit: 'Hz', log: true, offAtZero: true },
    ...band('low_shelf', 'Graves', 20, 1000, false),
    ...band('mid1', 'Medios 1', 40, 16000),
    ...band('mid2', 'Medios 2', 40, 16000),
    ...band('high_shelf', 'Agudos', 1000, 20000, false),
    { path: 'lowpass', label: 'Paso bajo', min: 1000, max: 20000, step: 10, unit: 'Hz', log: true, offAtZero: true },
  ],
  compressor: [
    { path: 'threshold_db', label: 'Umbral', min: -60, max: 0, step: 0.5, unit: 'dB' },
    { path: 'ratio', label: 'Ratio', min: 1, max: 20, step: 0.1, unit: ':1', log: true },
    { path: 'attack_ms', label: 'Ataque', min: 0.1, max: 200, step: 0.1, unit: 'ms', log: true },
    { path: 'release_ms', label: 'Liberación', min: 5, max: 2000, step: 1, unit: 'ms', log: true },
    { path: 'knee_db', label: 'Rodilla', min: 0, max: 24, step: 0.5, unit: 'dB' },
    { path: 'makeup_db', label: 'Ganancia', min: 0, max: 24, step: 0.5, unit: 'dB' },
  ],
  reverb: [
    { path: 'room_size', label: 'Tamaño', min: 0, max: 1, step: 0.01, unit: '%', percent: true },
    { path: 'damping', label: 'Amortiguación', min: 0, max: 1, step: 0.01, unit: '%', percent: true },
    { path: 'width', label: 'Anchura', min: 0, max: 1, step: 0.01, unit: '%', percent: true },
    { path: 'predelay_ms', label: 'Pre-retardo', min: 0, max: 200, step: 1, unit: 'ms' },
    { path: 'mix', label: 'Mezcla', min: 0, max: 1, step: 0.01, unit: '%', percent: true },
  ],
  delay: [
    { path: 'time_ms', label: 'Tiempo', min: 1, max: 2000, step: 1, unit: 'ms', log: true },
    { path: 'feedback', label: 'Realimentación', min: 0, max: 0.95, step: 0.01, unit: '%', percent: true },
    { path: 'high_cut', label: 'Corte de agudos', min: 500, max: 20000, step: 10, unit: 'Hz', log: true },
    { path: 'mix', label: 'Mezcla', min: 0, max: 1, step: 0.01, unit: '%', percent: true },
  ],
  limiter: [
    { path: 'input_db', label: 'Entrada', min: 0, max: 24, step: 0.5, unit: 'dB' },
    { path: 'ceiling_db', label: 'Techo', min: -12, max: 0, step: 0.1, unit: 'dBFS' },
    { path: 'release_ms', label: 'Liberación', min: 5, max: 1000, step: 1, unit: 'ms', log: true },
  ],
}

export function getParam(p: EffectParams, path: string): number {
  const [a, b] = path.split('.')
  const v = (p as unknown as Record<string, unknown>)[a]
  return b ? ((v as Record<string, number>)[b] ?? 0) : (v as number)
}

export function setParam(p: EffectParams, path: string, value: number): EffectParams {
  const [a, b] = path.split('.')
  const obj = p as unknown as Record<string, unknown>
  if (!b) return { ...obj, [a]: value } as unknown as EffectParams
  return { ...obj, [a]: { ...(obj[a] as Record<string, number>), [b]: value } } as unknown as EffectParams
}

/** Posición del deslizador (0–1000) ↔ valor, con escala lineal o logarítmica. */
export function toSlider(c: Control, v: number): number {
  if (c.offAtZero && v <= 0) return 0
  const x = c.log ? Math.log(v / c.min) / Math.log(c.max / c.min) : (v - c.min) / (c.max - c.min)
  return Math.round(Math.max(0, Math.min(1, x)) * 1000)
}

export function fromSlider(c: Control, s: number): number {
  if (c.offAtZero && s === 0) return 0
  const x = s / 1000
  const v = c.log ? c.min * Math.pow(c.max / c.min, x) : c.min + x * (c.max - c.min)
  return Math.round(v / c.step) * c.step
}

export function formatValue(c: Control, v: number): string {
  if (c.offAtZero && v <= 0) return 'apagado'
  if (c.percent) return `${Math.round(v * 100)} %`
  if (c.unit === 'Hz' && v >= 1000) return `${(v / 1000).toFixed(v >= 10000 ? 1 : 2)} kHz`
  const digits = c.step >= 1 ? 0 : c.step >= 0.1 ? 1 : 2
  return `${v.toFixed(digits)}${c.unit === ':1' ? ':1' : c.unit ? ` ${c.unit}` : ''}`
}
