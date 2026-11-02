import { useRef, useState } from 'react'
import type { AutomationLane, AutoPoint } from '../lib/api'
import { fromNorm, toNorm, type TargetInfo } from '../lib/automation'

interface Props {
  lane: AutomationLane
  info: TargetInfo
  /** Duración de la línea de tiempo (s). */
  duration: number
  color: string
  locked: boolean
  snap: (t: number, pxPerSec: number, free: boolean) => number
  onPreview: (lane: AutomationLane) => void
  onCommit: (before: AutomationLane, after: AutomationLane) => void
}

/**
 * Editor de una curva de automatización: clic en el fondo añade un punto (y
 * se arrastra en el mismo gesto), arrastrar un punto lo mueve (Mayús: solo el
 * valor), doble clic o clic derecho lo borra.
 */
export function AutomationLaneView({ lane, info, duration, color, locked, snap, onPreview, onCommit }: Props) {
  const areaRef = useRef<HTMLDivElement>(null)
  const [drag, setDrag] = useState<{ index: number; label: string } | null>(null)
  const points = lane.points

  const geometry = () => {
    const r = areaRef.current!.getBoundingClientRect()
    // El área interior deja un margen arriba y abajo (CSS) para agarrar los puntos extremos.
    return { r, pxPerSec: r.width / duration, h: r.height }
  }
  const valueFromY = (clientY: number) => {
    const { r, h } = geometry()
    return fromNorm(info, 1 - (clientY - r.top) / h)
  }
  const timeFromX = (clientX: number) => {
    const { r } = geometry()
    return Math.max(0, Math.min(duration, ((clientX - r.left) / r.width) * duration))
  }

  /** Arrastra el punto `index` de `start` (lista ya ordenada) hasta soltar. */
  const beginDrag = (e: React.PointerEvent, start: AutoPoint[], index: number, before: AutomationLane) => {
    const el = areaRef.current!
    el.setPointerCapture(e.pointerId)
    const { pxPerSec } = geometry()
    const fixedTime = start[index].time
    let last: AutoPoint[] = start
    const update = (ev: PointerEvent) => {
      // El punto no puede adelantar a sus vecinos: así la curva conserva su orden.
      const lo = index > 0 ? start[index - 1].time : 0
      const hi = index < start.length - 1 ? start[index + 1].time : duration
      const time = ev.shiftKey ? fixedTime : Math.min(hi, Math.max(lo, snap(timeFromX(ev.clientX), pxPerSec, ev.altKey)))
      const value = valueFromY(ev.clientY)
      last = start.map((p, i) => (i === index ? { time, value } : p))
      setDrag({ index, label: `${info.format(value)} · ${time.toFixed(2)} s` })
      onPreview({ ...lane, points: last })
    }
    const up = () => {
      el.removeEventListener('pointermove', update)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      setDrag(null)
      onCommit(before, { ...lane, points: last })
    }
    el.addEventListener('pointermove', update)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  const onAreaDown = (e: React.PointerEvent) => {
    if (e.button !== 0 || locked) return
    const { pxPerSec } = geometry()
    const time = snap(timeFromX(e.clientX), pxPerSec, e.altKey)
    const p = { time, value: valueFromY(e.clientY) }
    const index = points.filter((q) => q.time <= time).length
    const next = [...points.slice(0, index), p, ...points.slice(index)]
    onPreview({ ...lane, points: next })
    beginDrag(e, next, index, lane)
  }

  const onPointDown = (e: React.PointerEvent, index: number) => {
    e.stopPropagation()
    if (e.button !== 0 || locked) return
    beginDrag(e, points, index, lane)
  }

  const removePoint = (e: React.MouseEvent, index: number) => {
    e.preventDefault()
    e.stopPropagation()
    if (locked) return
    onCommit(lane, { ...lane, points: points.filter((_, i) => i !== index) })
  }

  // La curva en coordenadas 0..1000 × 0..100 (el SVG se estira al tamaño del carril).
  const y = (v: number) => 100 - toNorm(info, v) * 100
  const x = (t: number) => (t / duration) * 1000
  const path =
    points.length === 0
      ? ''
      : [`0,${y(points[0].value)}`, ...points.map((p) => `${x(p.time)},${y(p.value)}`), `1000,${y(points[points.length - 1].value)}`].join(
          ' ',
        )

  return (
    <div
      className={`auto-area ${lane.enabled ? '' : 'disabled'}`}
      style={{ ['--auto-color' as string]: color }}
      onPointerDown={onAreaDown}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="auto-inner" ref={areaRef}>
        {points.length === 0 ? (
          <span className="auto-empty small muted">Haz clic para añadir el primer punto</span>
        ) : (
          <svg viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true">
            <polygon points={`0,100 ${path} 1000,100`} className="auto-fill" />
            <polyline points={path} className="auto-line" />
          </svg>
        )}
        {points.map((p, i) => (
          <span
            key={i}
            className={`auto-point ${drag?.index === i ? 'active' : ''}`}
            style={{ left: `${(p.time / duration) * 100}%`, top: `${100 - toNorm(info, p.value) * 100}%` }}
            title={`${info.format(p.value)} · ${p.time.toFixed(2)} s (doble clic o clic derecho: borrar)`}
            onPointerDown={(e) => onPointDown(e, i)}
            onDoubleClick={(e) => removePoint(e, i)}
            onContextMenu={(e) => removePoint(e, i)}
          />
        ))}
        {drag && points[drag.index] && (
          <span
            className="auto-tip small mono"
            style={{ left: `${(points[drag.index].time / duration) * 100}%`, top: `${100 - toNorm(info, points[drag.index].value) * 100}%` }}
          >
            {drag.label}
          </span>
        )}
      </div>
    </div>
  )
}
