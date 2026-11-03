import { useEffect, useState } from 'react'
import { api, errorMessage, type ModelInfo, type SeparationResult } from '../lib/api'
import type { SeparationJob } from '../lib/useSeparation'
import { Icon } from './Icon'
import { tr, trBackend } from '../i18n'

interface Props {
  job: SeparationJob | null
  lastResult: SeparationResult | null
  workerReady: boolean
  onStart: (modelId: string, device: string) => void
  onCancel: () => void
  onOpenModels: () => void
  onError: (m: string) => void
}

export function SeparationPanel({ job, lastResult, workerReady, onStart, onCancel, onOpenModels, onError }: Props) {
  const [models, setModels] = useState<ModelInfo[] | null>(null)
  const [modelId, setModelId] = useState('')
  const [device, setDevice] = useState('auto')

  useEffect(() => {
    if (!workerReady) return
    api
      .worker<{ models: ModelInfo[] }>('models.list')
      .then((r) => {
        const installed = r.models.filter((m) => m.task === 'separation' && m.installed)
        setModels(installed)
        setModelId((cur) => (installed.some((m) => m.id === cur) ? cur : (installed[0]?.id ?? '')))
      })
      .catch((e) => onError(errorMessage(e)))
  }, [workerReady, onError])

  if (job) {
    return (
      <div className="panel">
        <div className="panel-row">
          <strong>{tr('Separando con')} {job.modelId}</strong>
          <span className="muted small">{job.message}</span>
          <button className="btn ghost small" onClick={onCancel} disabled={!job.jobId || job.progress >= 1}>
            <Icon name="x" size={14} /> {tr('Cancelar')}
          </button>
        </div>
        <div className="progress">
          <div style={{ width: `${job.progress * 100}%` }} />
        </div>
      </div>
    )
  }

  if (!workerReady) return <div className="panel muted">{tr('El proceso de IA se está iniciando…')}</div>

  if (models && models.length === 0) {
    return (
      <div className="panel panel-row">
        <span>{tr('Para separar stems necesitas descargar un modelo de separación (una sola vez).')}</span>
        <button className="btn primary" onClick={onOpenModels}>
          <Icon name="download" size={16} /> {tr('Ver modelos')}
        </button>
      </div>
    )
  }

  const selected = models?.find((m) => m.id === modelId)
  return (
    <div className="panel">
      <div className="panel-row">
        <label className="field">
          <span className="small muted">{tr('Modelo')}</span>
          <select value={modelId} onChange={(e) => setModelId(e.target.value)}>
            {models?.map((m) => (
              <option key={m.id} value={m.id}>
                {trBackend(m.name)}
              </option>
            ))}
          </select>
        </label>
        <label className="field">
          <span className="small muted">{tr('Procesar con')}</span>
          <select value={device} onChange={(e) => setDevice(e.target.value)}>
            <option value="auto">{tr('Automático (GPU si está disponible)')}</option>
            <option value="cpu">CPU</option>
          </select>
        </label>
        <button className="btn primary" disabled={!modelId} onClick={() => onStart(modelId, device)}>
          <Icon name="model" size={16} /> {tr('Separar')}
        </button>
      </div>
      {selected?.stems && <p className="small muted">Stems: {selected.stems.join(' · ')}</p>}
      {lastResult && (
        <p className="small muted">
          Última separación: {lastResult.duration.toFixed(0)} s de audio en {lastResult.seconds.toFixed(1)} s (
          {lastResult.device.toUpperCase()}).
        </p>
      )}
    </div>
  )
}
