import { useEffect, useRef } from 'react'
import type { Peaks } from '../lib/api'

interface Props {
  peaks: Peaks
  /** Segundo del audio que se dibuja en el borde izquierdo. */
  from?: number
  /** Segundos de audio que ocupan todo el ancho. */
  span: number
  sampleRate: number
  color: string
  dimmed?: boolean
}

/** Dibuja los picos en un canvas; se redibuja solo al cambiar tamaño o datos. */
/** Ancho máximo del lienzo en píxeles físicos (los navegadores limitan su tamaño). */
const MAX_CANVAS = 16384

export function Waveform({ peaks, from = 0, span, sampleRate, color, dimmed }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const draw = () => {
      const dpr = window.devicePixelRatio || 1
      const w = canvas.clientWidth
      const h = canvas.clientHeight
      if (w === 0 || h === 0) return
      const peaksPerSecond = sampleRate / peaks.frames_per_peak
      // Más resolución que picos no aporta nada: se limita el lienzo y se escala.
      const sx = Math.min(dpr, MAX_CANVAS / w, Math.max(1, (span * peaksPerSecond * 2) / w))
      canvas.width = Math.max(1, Math.round(w * sx))
      canvas.height = Math.round(h * dpr)
      const ctx = canvas.getContext('2d')
      if (!ctx) return
      ctx.setTransform(sx, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, w, h)

      const resolved = getComputedStyle(canvas).getPropertyValue('--wave-color').trim() || '#2ee6c5'
      ctx.fillStyle = resolved
      const count = peaks.data.length / 2
      const totalPeaks = Math.max(1e-6, span * peaksPerSecond)
      const shift = from * peaksPerSecond
      const mid = h / 2
      const cols = canvas.width
      const colW = w / cols
      for (let col = 0; col < cols; col++) {
        const x = col * colW
        const start = Math.floor((col / cols) * totalPeaks + shift)
        const end = Math.max(start + 1, Math.floor(((col + 1) / cols) * totalPeaks + shift))
        if (end <= 0) continue
        if (start >= count) break
        let lo = 0
        let hi = 0
        for (let i = Math.max(0, start); i < Math.min(end, count); i++) {
          lo = Math.min(lo, peaks.data[i * 2])
          hi = Math.max(hi, peaks.data[i * 2 + 1])
        }
        const top = mid - hi * mid * 0.95
        const bottom = mid - lo * mid * 0.95
        ctx.fillRect(x, top, Math.max(colW, 1 / sx), Math.max(1 / dpr, bottom - top))
      }
    }

    draw()
    const ro = new ResizeObserver(draw)
    ro.observe(canvas)
    return () => ro.disconnect()
  }, [peaks, from, span, sampleRate])

  return (
    <canvas
      ref={canvasRef}
      className="waveform"
      style={{ ['--wave-color' as string]: color, opacity: dimmed ? 0.3 : 1 }}
    />
  )
}
