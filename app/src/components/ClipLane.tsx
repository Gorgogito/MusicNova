import { useRef } from 'react'
import type { Clip, TrackView } from '../lib/api'
import { clipEnd, fadeCurve, moveClip, replaceClip, setFadeIn, setFadeOut, trimEnd, trimStart } from '../lib/clips'
import { Waveform } from './Waveform'

type Mode = 'move' | 'trim-start' | 'trim-end' | 'fade-in' | 'fade-out'

interface Props {
  track: TrackView
  /** Duración de la línea de tiempo (s). */
  duration: number
  sampleRate: number
  color: string
  dimmed: boolean
  selected: string | null
  /** Bloquea la edición (p. ej. mientras se graba). */
  locked: boolean
  /** Ajusta un instante a la rejilla (`px` = píxeles por segundo). */
  snap: (t: number, pxPerSec: number, free: boolean) => number
  onSelect: (clipId: string | null) => void
  onSeek: (t: number) => void
  /** Vista previa durante el arrastre (sin guardar). */
  onPreview: (clips: Clip[]) => void
  /** Fin del arrastre: guarda y apunta en el historial. */
  onCommit: (before: Clip[], after: Clip[]) => void
}

const CURVE = fadeCurve()
const EDGE_PX = 7
const HANDLE_PX = 7

const MODE_CURSOR: Record<Mode, string> = {
  move: 'grab',
  'trim-start': 'w-resize',
  'trim-end': 'e-resize',
  'fade-in': 'ew-resize',
  'fade-out': 'ew-resize',
}

/** Zona del clip bajo el puntero: tiradores de fundido arriba, bordes para recortar y el resto para mover. */
function hitMode(c: Clip, el: HTMLElement, clientX: number, clientY: number): Mode {
  const r = el.getBoundingClientRect()
  const x = clientX - r.left
  const y = clientY - r.top
  const pps = r.width / c.length
  if (y < 14) {
    if (Math.abs(x - c.fade_in * pps) <= HANDLE_PX) return 'fade-in'
    if (Math.abs(r.width - c.fade_out * pps - x) <= HANDLE_PX) return 'fade-out'
  }
  const edge = Math.min(EDGE_PX, r.width / 4)
  if (x <= edge) return 'trim-start'
  if (x >= r.width - edge) return 'trim-end'
  return 'move'
}

/** Área sombreada encima de la curva de fundido (coordenadas 0..1000 × 0..100). */
function fadePath(width: number, outgoing: boolean): string {
  if (width <= 0) return ''
  const pts = CURVE.map((g, i) => {
    const x = (i / (CURVE.length - 1)) * width
    return `${outgoing ? 1000 - x : x},${(100 - g * 100).toFixed(2)}`
  })
  const edge = outgoing ? 1000 : 0
  return `M${edge},0 L${edge},100 L${pts.join(' L')} L${outgoing ? 1000 - width : width},0 Z`
}

export function ClipLane(props: Props) {
  const { track, duration, selected, locked } = props
  const areaRef = useRef<HTMLDivElement>(null)
  const pct = (t: number) => `${(t / duration) * 100}%`

  const timeAt = (clientX: number) => {
    const r = areaRef.current!.getBoundingClientRect()
    return Math.max(0, Math.min(duration, ((clientX - r.left) / r.width) * duration))
  }

  const onAreaDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return
    props.onSelect(null)
    props.onSeek(timeAt(e.clientX))
  }

  const onClipDown = (e: React.PointerEvent<HTMLDivElement>, clip: Clip) => {
    if (e.button !== 0) return
    e.stopPropagation()
    const el = e.currentTarget
    const before = track.clips
    if (locked) {
      props.onSelect(clip.id)
      return
    }
    const mode = hitMode(clip, el, e.clientX, e.clientY)
    const pxPerSec = areaRef.current!.getBoundingClientRect().width / duration
    const x0 = e.clientX
    let moved = false
    let last = before
    el.setPointerCapture(e.pointerId)
    props.onSelect(clip.id)

    const update = (ev: PointerEvent) => {
      const dx = ev.clientX - x0
      if (!moved && Math.abs(dx) < 3) return
      moved = true
      const dt = dx / pxPerSec
      const snap = (t: number) => props.snap(t, pxPerSec, ev.altKey)
      let c: Clip
      switch (mode) {
        case 'move':
          c = moveClip(clip, snap(clip.start + dt))
          break
        case 'trim-start':
          c = trimStart(clip, snap(clip.start + dt))
          break
        case 'trim-end':
          c = trimEnd(clip, snap(clipEnd(clip) + dt), track.duration)
          break
        case 'fade-in':
          c = setFadeIn(clip, clip.fade_in + dt)
          break
        case 'fade-out':
          c = setFadeOut(clip, clip.fade_out - dt)
          break
      }
      last = replaceClip(before, c)
      props.onPreview(last)
    }
    const up = (ev: PointerEvent) => {
      el.removeEventListener('pointermove', update)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      el.style.cursor = ''
      if (moved) props.onCommit(before, last)
      else if (ev.type === 'pointerup') props.onSeek(timeAt(ev.clientX))
    }
    el.style.cursor = mode === 'move' ? 'grabbing' : MODE_CURSOR[mode]
    el.addEventListener('pointermove', update)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  const onClipHover = (e: React.PointerEvent<HTMLDivElement>, clip: Clip) => {
    if (e.buttons !== 0 || locked) return
    e.currentTarget.style.cursor = MODE_CURSOR[hitMode(clip, e.currentTarget, e.clientX, e.clientY)]
  }

  return (
    <div className="lane-wave" ref={areaRef} onPointerDown={onAreaDown}>
      {track.clips.map((c) => {
        const fi = (c.fade_in / c.length) * 1000
        const fo = (c.fade_out / c.length) * 1000
        return (
          <div
            key={c.id}
            className={`clip ${selected === c.id ? 'selected' : ''} ${props.dimmed ? 'dimmed' : ''}`}
            style={{ left: pct(c.start), width: pct(c.length), ['--clip-color' as string]: props.color }}
            onPointerDown={(e) => onClipDown(e, c)}
            onPointerMove={(e) => onClipHover(e, c)}
            title={`${c.start.toFixed(2)} s – ${clipEnd(c).toFixed(2)} s${c.fade_in || c.fade_out ? ` · fundidos ${c.fade_in.toFixed(2)} / ${c.fade_out.toFixed(2)} s` : ''}`}
          >
            <Waveform peaks={track.peaks} from={c.source_start} span={c.length} sampleRate={props.sampleRate} color={props.color} />
            {(fi > 0 || fo > 0) && (
              <svg className="clip-fades" viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true">
                {fi > 0 && <path d={fadePath(fi, false)} />}
                {fo > 0 && <path d={fadePath(fo, true)} />}
              </svg>
            )}
            {!locked && (
              <>
                <span className="fade-handle" style={{ left: `${fi / 10}%` }} />
                <span className="fade-handle" style={{ left: `${100 - fo / 10}%` }} />
              </>
            )}
          </div>
        )
      })}
    </div>
  )
}
