import { useState } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type ExportFormat, type ProjectView } from '../lib/api'
import { Icon } from './Icon'

const FORMATS: { id: ExportFormat; label: string; ext: string }[] = [
  { id: 'wav24', label: 'WAV 24 bits', ext: 'wav' },
  { id: 'wav16', label: 'WAV 16 bits (con dither)', ext: 'wav' },
  { id: 'wav32f', label: 'WAV 32 bits flotante', ext: 'wav' },
  { id: 'flac24', label: 'FLAC 24 bits (sin pérdida)', ext: 'flac' },
  { id: 'flac16', label: 'FLAC 16 bits (sin pérdida)', ext: 'flac' },
  { id: 'mp3_320', label: 'MP3 320 kbps', ext: 'mp3' },
  { id: 'mp3_256', label: 'MP3 256 kbps', ext: 'mp3' },
  { id: 'mp3_192', label: 'MP3 192 kbps', ext: 'mp3' },
  { id: 'mp3_128', label: 'MP3 128 kbps', ext: 'mp3' },
]

export function ExportPanel({ project, onError }: { project: ProjectView; onError: (m: string) => void }) {
  const [format, setFormat] = useState<ExportFormat>('wav24')
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const hasStems = project.tracks.some((t) => t.kind !== 'mix')

  const run = async (fn: () => Promise<string | null>) => {
    setBusy(true)
    setStatus(null)
    try {
      setStatus(await fn())
    } catch (e) {
      onError(errorMessage(e))
    } finally {
      setBusy(false)
    }
  }

  const exportMix = () =>
    run(async () => {
      const ext = FORMATS.find((f) => f.id === format)?.ext ?? 'wav'
      const path = await save({
        defaultPath: `${project.project.name} (mezcla).${ext}`,
        filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
      })
      if (!path) return null
      const r = await api.exportMix(path, format)
      const clip = r.peak_db > -0.1 ? ' ⚠ La mezcla satura: baja el volumen de algunas pistas.' : ''
      return `Mezcla exportada (pico ${r.peak_db.toFixed(1)} dBFS, ${r.sample_rate} Hz).${clip}`
    })

  const exportStems = () =>
    run(async () => {
      const dir = await open({ directory: true, title: 'Carpeta de destino para los stems' })
      if (typeof dir !== 'string') return null
      const files = await api.exportStems(dir, format)
      return `${files.length} ${files.length === 1 ? 'archivo exportado' : 'archivos exportados'} en ${dir}.`
    })

  return (
    <div className="panel">
      <div className="panel-row">
        <label className="field">
          <span className="small muted">Formato</span>
          <select value={format} onChange={(e) => setFormat(e.target.value as ExportFormat)}>
            {FORMATS.map((f) => (
              <option key={f.id} value={f.id}>
                {f.label}
              </option>
            ))}
          </select>
        </label>
        <button className="btn" onClick={exportMix} disabled={busy}>
          <Icon name="download" size={16} /> Exportar mezcla
        </button>
        <button className="btn" onClick={exportStems} disabled={busy}>
          <Icon name="download" size={16} /> {hasStems ? 'Exportar stems' : 'Exportar pista'}
        </button>
      </div>
      <p className="small muted">
        La mezcla respeta volumen, panorama, silencio y solo. Los stems se exportan tal cual, a su frecuencia
        original.
      </p>
      {status && <p className="small">{status}</p>}
    </div>
  )
}
