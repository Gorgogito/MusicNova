// Descripción de un parámetro numérico para <ParamControl>: escala del
// deslizador, pasos de los botones / teclas / rueda, formato y lectura del texto
// que escribe el usuario.

export interface ParamSpec {
  min: number
  max: number
  /** Valor al que vuelve con doble clic. */
  def?: number
  /** Valor → posición del deslizador (0–1). */
  toNorm: (v: number) => number
  /** Posición del deslizador (0–1) → valor, ya redondeado. */
  fromNorm: (n: number) => number
  /** Un paso hacia arriba (+1) o abajo (−1); `big` con Shift o Re Pág / Av Pág. */
  nudge: (v: number, dir: 1 | -1, big: boolean) => number
  format: (v: number) => string
  /** Texto escrito → valor; `null` si no se entiende. Puede ser asíncrono (plugins). */
  parse: (text: string) => number | null | Promise<number | null>
}

export const clamp01 = (x: number) => Math.max(0, Math.min(1, x))

export const roundTo = (v: number, step: number) => {
  const r = Math.round(v / step) * step
  // Evita 0.30000000000000004 en la pantalla y en el proyecto.
  const digits = Math.max(0, -Math.floor(Math.log10(step)) + 1)
  return Number(r.toFixed(Math.min(digits, 10)))
}

/**
 * Lee un número de un texto como «-3,5 dB», «1.5k», «1,2 kHz», «250ms» o «3:1».
 * Acepta coma o punto decimal y el sufijo k (miles). `null` si no hay número.
 */
export function parseNumber(text: string): number | null {
  const t = text.trim().toLowerCase().replace(',', '.').replace('−', '-')
  const m = t.match(/^([+-]?(?:\d+\.?\d*|\.\d+))\s*(k(?!b))?/)
  if (!m) return null
  const x = parseFloat(m[1]) * (m[2] ? 1000 : 1)
  return Number.isFinite(x) ? x : null
}

interface LinearOpts {
  min: number
  max: number
  step: number
  def?: number
  /** Pasos por cada paso grande (Shift). */
  big?: number
  format: (v: number) => string
  parse?: (text: string) => number | null
}

/** Parámetro lineal con paso fijo (volumen, panorama, ganancia...). */
export function linearSpec(o: LinearOpts): ParamSpec {
  const fix = (v: number) => roundTo(Math.max(o.min, Math.min(o.max, v)), o.step)
  return {
    min: o.min,
    max: o.max,
    def: o.def,
    toNorm: (v) => clamp01((v - o.min) / (o.max - o.min)),
    fromNorm: (n) => fix(o.min + clamp01(n) * (o.max - o.min)),
    nudge: (v, dir, big) => fix(v + dir * o.step * (big ? (o.big ?? 10) : 1)),
    format: o.format,
    parse: (text) => {
      const v = (o.parse ?? parseNumber)(text)
      return v == null ? null : fix(v)
    },
  }
}

// ---------- Mezclador ----------

/** Volumen de pista: -60 dB es silencio (-∞). Shift: pasos de 3 dB. */
export const VOLUME_SPEC = linearSpec({
  min: -60,
  max: 12,
  step: 0.5,
  def: 0,
  big: 6,
  format: (v) => (v <= -60 ? '-∞' : v.toFixed(1)),
  parse: (t) => (/^\s*-?\s*(∞|inf)/i.test(t) ? -60 : parseNumber(t)),
})

/** Panorama -1 (izquierda) … 1 (derecha); se muestra y se escribe como I50 / C / D50 (o -50 / 0 / 50). */
export const PAN_SPEC = linearSpec({
  min: -1,
  max: 1,
  step: 0.02,
  def: 0,
  big: 5,
  format: (v) => (Math.abs(v) < 0.01 ? 'C' : `${v < 0 ? 'I' : 'D'}${Math.round(Math.abs(v) * 100)}`),
  parse: (t) => {
    const s = t.trim().toLowerCase()
    if (s === 'c' || s === '') return 0
    const side = s.match(/^([idlr])\s*(.*)$/)
    const n = parseNumber(side ? side[2] : s)
    if (n == null) return null
    if (!side) return n / 100
    return ((side[1] === 'i' || side[1] === 'l' ? -1 : 1) * Math.abs(n)) / 100
  },
})

/** Ganancia del máster (dB). */
export const MASTER_GAIN_SPEC = linearSpec({ min: -24, max: 12, step: 0.5, def: 0, big: 6, format: (v) => `${v.toFixed(1)} dB` })
