import { useEffect, useRef, useState } from 'react'
import { clamp01, type ParamSpec } from '../lib/param'
import { tr } from '../i18n'

interface Props {
  spec: ParamSpec
  value: number
  onChange: (v: number) => void
  disabled?: boolean
  /** Nombre del parámetro (accesibilidad y ayudas). */
  label: string
  /** Sin botones − / + (cabeceras de pista estrechas). */
  compact?: boolean
  /** Empieza / termina una edición continua (arrastre, botón mantenido, texto). */
  onEditStart?: () => void
  onEditEnd?: () => void
}

/** Arrastre normal: el valor sigue al ratón. Con Shift: 10 veces más lento. */
const FINE = 0.1
/** Mantener pulsado − / +: primera repetición y ritmo (se acelera). */
const REPEAT_DELAY = 400
const REPEAT_START = 110
const REPEAT_MIN = 30

/**
 * Control de un parámetro: deslizador, valor editable y botones − / +.
 *
 * - Arrastrar mueve el valor; con Shift, ajuste fino. Doble clic: valor por defecto.
 * - Flechas (Shift: paso grande), Re Pág / Av Pág, Inicio / Fin con el foco en el deslizador.
 * - Ctrl + rueda sobre el control: paso a paso (sin Ctrl la rueda desplaza la página).
 * - Clic en el valor: escribirlo («120», «1.5k», «-3 dB»); Intro aplica, Esc cancela.
 * - − / +: un paso; mantenidos, se repiten cada vez más rápido.
 */
export function ParamControl({ spec, value, onChange, disabled, label, compact, onEditStart, onEditEnd }: Props) {
  // El último valor enviado: el padre puede tardar en devolverlo (repeticiones, rueda).
  const current = useRef(value)
  useEffect(() => {
    current.current = value
  }, [value])
  const latest = useRef({ spec, onChange, onEditStart, onEditEnd })
  latest.current = { spec, onChange, onEditStart, onEditEnd }

  const emit = (v: number) => {
    const s = latest.current.spec
    const next = Math.max(s.min, Math.min(s.max, v))
    if (Number.isNaN(next) || next === current.current) return
    current.current = next
    latest.current.onChange(next)
  }
  const step = (dir: 1 | -1, big: boolean) => emit(latest.current.spec.nudge(current.current, dir, big))

  // ---------- Deslizador ----------
  const [dragging, setDragging] = useState(false)

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (disabled || e.button !== 0) return
    const el = e.currentTarget
    e.preventDefault()
    el.focus()
    el.setPointerCapture(e.pointerId)
    const rect = el.getBoundingClientRect()
    const at = (x: number) => clamp01((x - rect.left) / rect.width)
    // Con Shift no salta al punto del clic: se mueve desde donde está.
    let norm = e.shiftKey ? spec.toNorm(current.current) : at(e.clientX)
    let lastX = e.clientX
    if (!e.shiftKey) emit(spec.fromNorm(norm))
    setDragging(true)
    onEditStart?.()
    const move = (ev: PointerEvent) => {
      norm = ev.shiftKey ? clamp01(norm + ((ev.clientX - lastX) / rect.width) * FINE) : at(ev.clientX)
      lastX = ev.clientX
      emit(latest.current.spec.fromNorm(norm))
    }
    const up = () => {
      el.removeEventListener('pointermove', move)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      setDragging(false)
      latest.current.onEditEnd?.()
    }
    el.addEventListener('pointermove', move)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (disabled) return
    const s = latest.current.spec
    const keys: Record<string, () => void> = {
      ArrowRight: () => step(1, e.shiftKey),
      ArrowUp: () => step(1, e.shiftKey),
      ArrowLeft: () => step(-1, e.shiftKey),
      ArrowDown: () => step(-1, e.shiftKey),
      PageUp: () => step(1, true),
      PageDown: () => step(-1, true),
      Home: () => emit(s.min),
      End: () => emit(s.max),
    }
    const run = keys[e.key]
    if (!run) return
    // Sin esto las flechas también moverían el cabezal (atajos globales).
    e.preventDefault()
    e.stopPropagation()
    run()
  }

  const resetToDefault = () => {
    if (!disabled && spec.def != null) emit(spec.def)
  }

  // Ctrl + rueda: el listener no puede ser pasivo para evitar el zoom / desplazamiento.
  const root = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = root.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey || disabled) return
      e.preventDefault()
      e.stopPropagation()
      const d = e.deltaY || e.deltaX
      if (d) step(d < 0 ? 1 : -1, e.shiftKey)
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [disabled])

  // ---------- Botones − / + (con repetición) ----------
  const repeat = useRef<number | null>(null)
  const stopRepeat = () => {
    if (repeat.current == null) return
    window.clearTimeout(repeat.current)
    repeat.current = null
    latest.current.onEditEnd?.()
  }
  useEffect(() => stopRepeat, [])
  const startRepeat = (e: React.PointerEvent, dir: 1 | -1) => {
    if (disabled || e.button !== 0) return
    e.preventDefault()
    const big = e.shiftKey
    onEditStart?.()
    step(dir, big)
    let delay = REPEAT_START
    const tick = () => {
      step(dir, big)
      delay = Math.max(REPEAT_MIN, delay * 0.85)
      repeat.current = window.setTimeout(tick, delay)
    }
    repeat.current = window.setTimeout(tick, REPEAT_DELAY)
  }
  const stepButton = (dir: 1 | -1) => (
    <button
      type="button"
      className="param-step"
      tabIndex={-1}
      disabled={disabled || (dir > 0 ? value >= spec.max : value <= spec.min)}
      title={dir > 0 ? tr('Aumentar (mantén pulsado para repetir; Shift: paso grande)') : tr('Disminuir (mantén pulsado para repetir; Shift: paso grande)')}
      aria-label={dir > 0 ? tr('Aumentar') : tr('Disminuir')}
      onPointerDown={(e) => startRepeat(e, dir)}
      onPointerUp={stopRepeat}
      onPointerLeave={stopRepeat}
      onPointerCancel={stopRepeat}
    >
      {dir > 0 ? '+' : '−'}
    </button>
  )

  // ---------- Valor editable ----------
  const [editing, setEditing] = useState<string | null>(null)
  const [invalid, setInvalid] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (editing != null) input.current?.select()
    // Solo al abrir el campo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing != null])
  // Copia síncrona del texto: Intro, Esc y la pérdida del foco no deben aplicar dos veces.
  const draft = useRef<string | null>(null)
  const startEditing = () => {
    if (disabled) return
    const text = spec.format(value)
    draft.current = text
    setInvalid(false)
    setEditing(text)
    onEditStart?.()
  }
  /** `apply`: leer el texto; `blur`: si no se entiende, se cancela en vez de avisar. */
  const finishEditing = async (apply: boolean, blur = false) => {
    const text = draft.current
    if (text == null) return
    draft.current = null
    if (apply && text.trim() !== spec.format(value)) {
      const v = await spec.parse(text)
      if (v == null || Number.isNaN(v)) {
        if (!blur) {
          draft.current = text
          setInvalid(true)
          return
        }
      } else emit(v)
    }
    setEditing(null)
    setInvalid(false)
    latest.current.onEditEnd?.()
  }

  const pct = clamp01(spec.toNorm(value)) * 100
  return (
    <div ref={root} className={`param ${compact ? 'compact' : ''} ${disabled ? 'disabled' : ''}`}>
      <div
        className={`param-slider ${dragging ? 'dragging' : ''}`}
        role="slider"
        tabIndex={disabled ? -1 : 0}
        aria-label={label}
        aria-valuemin={spec.min}
        aria-valuemax={spec.max}
        aria-valuenow={value}
        aria-valuetext={spec.format(value)}
        aria-disabled={disabled}
        title={tr('Arrastra (Shift: ajuste fino) · doble clic: valor por defecto · flechas o Ctrl + rueda: paso a paso')}
        onPointerDown={onPointerDown}
        onDoubleClick={resetToDefault}
        onKeyDown={onKeyDown}
      >
        <div className="param-rail">
          <div className="param-fill" style={{ width: `${pct}%` }} />
        </div>
        <div className="param-thumb" style={{ left: `${pct}%` }} />
      </div>
      <div className="param-value">
        {!compact && stepButton(-1)}
        {editing != null ? (
          <input
            ref={input}
            type="text"
            className={`param-input mono small ${invalid ? 'invalid' : ''}`}
            value={editing}
            aria-label={label}
            title={invalid ? tr('No se entiende ese valor') : tr('Intro: aplicar · Esc: cancelar')}
            onChange={(e) => {
              draft.current = e.target.value
              setEditing(e.target.value)
              setInvalid(false)
            }}
            onKeyDown={(e) => {
              e.stopPropagation()
              if (e.key === 'Enter') finishEditing(true)
              else if (e.key === 'Escape') finishEditing(false)
            }}
            onBlur={() => finishEditing(true, true)}
          />
        ) : (
          <button
            type="button"
            className="param-text mono small"
            disabled={disabled}
            title={tr('Clic para escribir el valor')}
            onClick={startEditing}
          >
            {spec.format(value)}
          </button>
        )}
        {!compact && stepButton(1)}
      </div>
    </div>
  )
}
