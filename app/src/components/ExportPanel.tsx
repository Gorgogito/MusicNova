import { useState } from 'react'
import { open, save } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type ExportFormat, type ProjectView } from '../lib/api'
import { Icon } from './Icon'
import { tr, plural } from '../i18n'

const FORMATS: { id: ExportFormat; label: string; ext: string }[] = [
  { id: 'wav24', label: tr('WAV 24 bits'), ext: 'wav' },
  { id: 'wav16', label: tr('WAV 16 bits (con dither)'), ext: 'wav' },
  { id: 'wav32f', label: tr('WAV 32 bits flotante'), ext: 'wav' },
  { id: 'flac24', label: tr('FLAC 24 bits (sin pérdida)'), ext: 'flac' },
  { id: 'flac16', label: tr('FLAC 16 bits (sin pérdida)'), ext: 'flac' },
  { id: 'mp3_320', label: tr('MP3 320 kbps'), ext: 'mp3' },
  { id: 'mp3_256', label: tr('MP3 256 kbps'), ext: 'mp3' },
  { id: 'mp3_192', label: tr('MP3 192 kbps'), ext: 'mp3' },
  { id: 'mp3_128', label: tr('MP3 128 kbps'), ext: 'mp3' },
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
        defaultPath: `${project.project.name} (${tr('mezcla')}).${ext}`,
        filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
      })
      if (!path) return null
      const r = await api.exportMix(path, format)
      const tp = r.true_peak_db ?? r.peak_db
      const clip = tp > -0.1 ? tr(' ⚠ Satura: baja el volumen o añade un limitador en el máster (Mezcla).') : ''
      const loud = r.lufs != null ? `${r.lufs.toFixed(1)} LUFS, LRA ${r.lra.toFixed(1)} LU, ` : ''
      return tr('Mezcla exportada ({loud}pico real {tp} dBTP, {rate} Hz).', { loud, tp: tp.toFixed(1), rate: r.sample_rate }) + clip
    })

  const exportStems = () =>
    run(async () => {
      const dir = await open({ directory: true, title: tr('Carpeta de destino para los stems') })
      if (typeof dir !== 'string') return null
      const files = await api.exportStems(dir, format)
      return plural(files.length, '{n} archivo exportado en {dir}.', '{n} archivos exportados en {dir}.', { dir })
    })

  return (
    <div className="panel">
      <div className="panel-row">
        <label className="field">
          <span className="small muted">{tr('Formato')}</span>
          <select value={format} onChange={(e) => setFormat(e.target.value as ExportFormat)}>
            {FORMATS.map((f) => (
              <option key={f.id} value={f.id}>
                {f.label}
              </option>
            ))}
          </select>
        </label>
        <button className="btn" onClick={exportMix} disabled={busy}>
          <Icon name="download" size={16} /> {tr('Exportar mezcla')}
        </button>
        <button className="btn" onClick={exportStems} disabled={busy}>
          <Icon name="download" size={16} /> {hasStems ? tr('Exportar stems') : tr('Exportar pista')}
        </button>
      </div>
      <p className="small muted">
        {tr('La mezcla respeta volumen, panorama, silencio y solo. Los stems se exportan tal cual, a su frecuencia original.')}
      </p>
      {status && <p className="small">{status}</p>}
    </div>
  )
}
