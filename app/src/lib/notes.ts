// Notas transcritas (piano roll) y medidas de afinación a partir de la curva de tono.
import type { MusicalKey, Note, PitchCurve } from './api'
import { mod12, noteName, type Notation } from './chords'

/** Duración mínima de una nota editada (s); el núcleo admite 0,02. */
export const MIN_NOTE = 0.03

/** «Do4», «C#5»… (Do central = 60 = Do4). */
export function noteLabel(pitch: number, notation: Notation, flats = false): string {
  return `${noteName(mod12(pitch), notation, flats)}${Math.floor(pitch / 12) - 1}`
}

export const isBlackKey = (pitch: number) => [1, 3, 6, 8, 10].includes(mod12(pitch))

/** Nota MIDI continua de cada punto de la curva (NaN sin voz). */
export function curveMidi(curve: PitchCurve): number[] {
  return curve.f0.map((f) => (f > 0 ? 69 + 12 * Math.log2(f / 440) : NaN))
}

/** Rango de notas a mostrar: el de las notas (y la curva), con margen y al menos una octava. */
export function pitchRange(notes: Note[], midi: number[] = []): [number, number] {
  let lo = Infinity
  let hi = -Infinity
  for (const n of notes) {
    lo = Math.min(lo, n.pitch)
    hi = Math.max(hi, n.pitch)
  }
  for (const m of midi) {
    if (Number.isFinite(m)) {
      lo = Math.min(lo, Math.floor(m))
      hi = Math.max(hi, Math.ceil(m))
    }
  }
  if (!Number.isFinite(lo)) return [48, 72]
  lo -= 2
  hi += 2
  if (hi - lo < 12) {
    const mid = (lo + hi) / 2
    lo = Math.floor(mid - 6)
    hi = lo + 12
  }
  return [Math.max(0, lo), Math.min(127, hi)]
}

/** Tramos seguidos con voz [inicio, fin) de la curva, para dibujarla. */
export function voicedRuns(midi: number[]): [number, number][] {
  const runs: [number, number][] = []
  let start = -1
  midi.forEach((m, i) => {
    if (Number.isFinite(m)) {
      if (start < 0) start = i
    } else if (start >= 0) {
      runs.push([start, i])
      start = -1
    }
  })
  if (start >= 0) runs.push([start, midi.length])
  return runs
}

const MAJOR = [0, 2, 4, 5, 7, 9, 11]
const MINOR = [0, 2, 3, 5, 7, 8, 10]

/** Clases de altura de la escala de una tonalidad (menor natural). */
export function scaleOf(key: Pick<MusicalKey, 'tonic' | 'mode'>): number[] {
  return (key.mode === 'major' ? MAJOR : MINOR).map((d) => mod12(key.tonic + d)).sort((a, b) => a - b)
}

/** Nota permitida más cercana (igual que el núcleo); escala vacía = cromática. */
export function nearestAllowed(p: number, scale: number[]): number {
  const base = Math.round(p)
  if (scale.length === 0) return base
  let best = base
  let dist = Infinity
  for (let d = -6; d <= 6; d++) {
    const n = base + d
    if (scale.includes(mod12(n)) && Math.abs(n - p) < dist) {
      dist = Math.abs(n - p)
      best = n
    }
  }
  return best
}

export interface TuningStats {
  voicedSeconds: number
  /** Fracción del tiempo cantado a ±25 cents de una nota de la escala. */
  inTune: number
  /** Fracción del tiempo por encima (+25) o por debajo (−25). */
  sharp: number
  flat: number
  /** Desviación media absoluta (cents). */
  meanCents: number
}

/** Mediana móvil de ±`radius` puntos dentro de cada tramo con voz (quita el vibrato). */
export function localCenter(midi: number[], radius = 20): number[] {
  const out = new Array<number>(midi.length).fill(NaN)
  for (const [a, b] of voicedRuns(midi)) {
    for (let i = a; i < b; i++) {
      const w: number[] = []
      for (let j = Math.max(a, i - radius); j < Math.min(b, i + radius + 1); j++) {
        if (Math.abs(midi[j] - midi[i]) <= 1.25) w.push(midi[j])
      }
      w.sort((x, y) => x - y)
      out[i] = w[w.length >> 1]
    }
  }
  return out
}

/** Afinación de la voz: desviación del centro local respecto a la nota de la escala más cercana. */
export function tuningStats(curve: PitchCurve, scale: number[]): TuningStats {
  const midi = curveMidi(curve).map((m, i) => ((curve.confidence[i] ?? 1) >= 0.3 ? m : NaN))
  const center = localCenter(midi)
  let n = 0
  let inTune = 0
  let sharp = 0
  let flat = 0
  let sum = 0
  for (const c of center) {
    if (!Number.isFinite(c)) continue
    const cents = (c - nearestAllowed(c, scale)) * 100
    n++
    sum += Math.abs(cents)
    if (cents > 25) sharp++
    else if (cents < -25) flat++
    else inTune++
  }
  return {
    voicedSeconds: n * curve.hop,
    inTune: n ? inTune / n : 0,
    sharp: n ? sharp / n : 0,
    flat: n ? flat / n : 0,
    meanCents: n ? sum / n : 0,
  }
}

// ---- Edición ----------------------------------------------------------------------

export function moveNote(n: Note, start: number, pitch: number): Note {
  const len = n.end - n.start
  const s = Math.max(0, start)
  return { ...n, start: s, end: s + len, pitch: Math.max(0, Math.min(127, Math.round(pitch))) }
}

export function resizeNote(n: Note, end: number): Note {
  return { ...n, end: Math.max(n.start + MIN_NOTE, end) }
}

export function sortNotes(notes: Note[]): Note[] {
  return [...notes].sort((a, b) => a.start - b.start || a.pitch - b.pitch)
}

/** Notas que suenan en el instante `t`. */
export function notesAt(notes: Note[], t: number): Note[] {
  return notes.filter((n) => n.start <= t && t < n.end)
}
