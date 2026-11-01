import { useEffect, useRef } from 'react'
import type { Peaks } from '../lib/api'

interface Props {
  peaks: Peaks
  /** Duración total de la línea de tiempo (la pista más larga). */
  timelineDuration: number
  sampleRate: number
  color: string
  dimmed?: boolean
}

/** Dibuja los picos en un canvas; se redibuja solo al cambiar tamaño o datos. */
export function Waveform({ peaks, timelineDuration, sampleRate, color, dimmed }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const draw = () => {
      const dpr = window.devicePixelRatio || 1
      const w = canvas.clientWidth
      const h = canvas.clientHeight
      if (w === 0 || h === 0) return
      canvas.width = Math.round(w * dpr)
      canvas.height = Math.round(h * dpr)
      const ctx = canvas.getContext('2d')
      if (!ctx) return
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, w, h)

      const resolved = getComputedStyle(canvas).getPropertyValue('--wave-color').trim() || '#2ee6c5'
      ctx.fillStyle = resolved
      const count = peaks.data.length / 2
      const peaksPerSecond = sampleRate / peaks.frames_per_peak
      const totalPeaks = Math.max(1, timelineDuration * peaksPerSecond)
      const mid = h / 2
      for (let x = 0; x < w; x++) {
        const start = Math.floor((x / w) * totalPeaks)
        const end = Math.max(start + 1, Math.floor(((x + 1) / w) * totalPeaks))
        if (start >= count) break
        let lo = 0
        let hi = 0
        for (let i = start; i < Math.min(end, count); i++) {
          lo = Math.min(lo, peaks.data[i * 2])
          hi = Math.max(hi, peaks.data[i * 2 + 1])
        }
        const top = mid - hi * mid * 0.95
        const bottom = mid - lo * mid * 0.95
        ctx.fillRect(x, top, 1, Math.max(1, bottom - top))
      }
    }

    draw()
    const ro = new ResizeObserver(draw)
    ro.observe(canvas)
    return () => ro.disconnect()
  }, [peaks, timelineDuration, sampleRate])

  return (
    <canvas
      ref={canvasRef}
      className="waveform"
      style={{ ['--wave-color' as string]: color, opacity: dimmed ? 0.3 : 1 }}
    />
  )
}
