// Automatización: destinos posibles de cada canal, rangos y utilidades de curva.
import type { AutomationLane, AutomationTarget, AutoPoint, EffectSlot } from './api'
import { CONTROLS, EFFECT_LABELS, formatValue, type Control } from './effects'

export interface TargetInfo {
  key: string
  target: AutomationTarget
  label: string
  min: number
  max: number
  /** Escala logarítmica (frecuencias, tiempos, ratio). */
  log: boolean
  format: (v: number) => string
}

export const targetKey = (t: AutomationTarget): string =>
  t.kind === 'effect' ? `fx:${t.slot}:${t.param}` : t.kind

const VOLUME: TargetInfo = {
  key: 'volume',
  target: { kind: 'volume' },
  label: 'Volumen',
  min: -60,
  max: 12,
  log: false,
  format: (v) => (v <= -60 ? '-∞ dB' : `${v.toFixed(1)} dB`),
}

const PAN: TargetInfo = {
  key: 'pan',
  target: { kind: 'pan' },
  label: 'Panorama',
  min: -1,
  max: 1,
  log: false,
  format: (v) => (Math.abs(v) < 0.01 ? 'Centro' : `${v < 0 ? 'Izq.' : 'Der.'} ${Math.round(Math.abs(v) * 100)} %`),
}

/** Destinos automatizables de un canal: volumen, panorama (no en el máster) y parámetros de sus efectos. */
export function channelTargets(effects: EffectSlot[], master: boolean): TargetInfo[] {
  const out = master ? [VOLUME] : [VOLUME, PAN]
  const seen: Record<string, number> = {}
  for (const slot of effects) {
    const type = slot.params.type
    seen[type] = (seen[type] ?? 0) + 1
    const name = `${EFFECT_LABELS[type]}${effects.filter((e) => e.params.type === type).length > 1 ? ` ${seen[type]}` : ''}`
    for (const c of CONTROLS[type]) {
      const control: Control = { ...c, offAtZero: false }
      const target: AutomationTarget = { kind: 'effect', slot: slot.id, param: c.path }
      out.push({
        key: targetKey(target),
        target,
        label: `${name} · ${c.label}`,
        min: c.min,
        max: c.max,
        log: !!c.log,
        format: (v) => formatValue(control, v),
      })
    }
  }
  return out
}

/** Valor ↔ posición vertical normalizada (0 = abajo, 1 = arriba). */
export function toNorm(info: TargetInfo, v: number): number {
  const x = info.log ? Math.log(v / info.min) / Math.log(info.max / info.min) : (v - info.min) / (info.max - info.min)
  return Math.max(0, Math.min(1, x))
}

export function fromNorm(info: TargetInfo, n: number): number {
  const x = Math.max(0, Math.min(1, n))
  return info.log ? info.min * Math.pow(info.max / info.min, x) : info.min + x * (info.max - info.min)
}

/** Valor de la curva en `t` (interpolación lineal; se mantiene fuera de los extremos). */
export function valueAt(points: AutoPoint[], t: number): number | null {
  if (points.length === 0) return null
  if (t <= points[0].time) return points[0].value
  for (let i = 1; i < points.length; i++) {
    const a = points[i - 1]
    const b = points[i]
    if (t < b.time) return b.time === a.time ? b.value : a.value + ((b.value - a.value) * (t - a.time)) / (b.time - a.time)
  }
  return points[points.length - 1].value
}

/** ¿Hay una curva activa que sustituya a este control? */
export function automated(lanes: AutomationLane[], kind: 'volume' | 'pan'): boolean {
  return lanes.some((l) => l.enabled && l.points.length > 0 && l.target.kind === kind)
}

/** Quita los carriles de efectos que ya no existen. */
export function pruneLanes(lanes: AutomationLane[], effects: EffectSlot[]): AutomationLane[] {
  return lanes.filter((l) => l.target.kind !== 'effect' || effects.some((e) => e.id === (l.target as { slot: string }).slot))
}

/** Carril nuevo con el primer destino libre; `null` si no queda ninguno. */
export function newLane(lanes: AutomationLane[], targets: TargetInfo[]): AutomationLane | null {
  const used = new Set(lanes.map((l) => targetKey(l.target)))
  const free = targets.find((t) => !used.has(t.key))
  return free ? { id: crypto.randomUUID(), target: free.target, enabled: true, points: [] } : null
}

/** Cambia el destino de un carril conservando la forma de la curva (valores reescalados). */
export function retarget(lane: AutomationLane, from: TargetInfo | undefined, to: TargetInfo): AutomationLane {
  const points = from
    ? lane.points.map((p) => ({ time: p.time, value: fromNorm(to, toNorm(from, p.value)) }))
    : []
  return { ...lane, target: to.target, points }
}
