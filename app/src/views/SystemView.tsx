import { useEffect, useState } from 'react'
import { api, errorMessage, type AppInfo, type SystemInfo, type WorkerStatus } from '../lib/api'
import { formatBytes } from '../lib/format'
import { Icon } from '../components/Icon'
import { open } from '@tauri-apps/plugin-dialog'
import { FORMAT_LABEL, scanPlugins, usePlugins } from '../lib/plugins'
import { formatDate } from '../lib/format'

/** Carpetas, escaneo y resultado de los plugins CLAP/VST3. */
function PluginsCard({ onError }: { onError: (m: string) => void }) {
  const { cache, scanning } = usePlugins()
  const [edited, setEdited] = useState<string[] | null>(null)
  const folders = edited ?? cache?.folders ?? []
  const [showFailed, setShowFailed] = useState(false)
  const count = (f: 'clap' | 'vst3') => cache?.plugins.filter((p) => p.format === f).length ?? 0

  const addFolder = async () => {
    const dir = await open({ directory: true, multiple: false })
    if (typeof dir === 'string' && !folders.includes(dir)) setEdited([...folders, dir])
  }

  return (
    <article className="card plugins-card">
      <div className="model-head">
        <h3>Plugins CLAP y VST3</h3>
        {cache?.scanned_at && <span className="badge">{cache.plugins.length} efectos</span>}
      </div>
      <p className="small muted">
        Se buscan en las carpetas estándar y en las que añadas. Cada archivo se examina en un proceso aparte, así que un
        plugin defectuoso no cierra MusicNova durante el escaneo. Al usarlos, los plugins se ejecutan dentro de la app.
      </p>
      <ul className="folder-list small">
        {(cache?.default_folders ?? []).map((f) => (
          <li key={f} className="muted mono">
            {f}
          </li>
        ))}
        {folders.map((f) => (
          <li key={f} className="mono">
            {f}
            <button className="icon-btn small" title="Quitar carpeta" onClick={() => setEdited(folders.filter((x) => x !== f))}>
              <Icon name="x" size={12} />
            </button>
          </li>
        ))}
      </ul>
      <div className="panel-row">
        <button className="btn ghost small" onClick={addFolder} disabled={!!scanning}>
          <Icon name="folder" size={14} /> Añadir carpeta
        </button>
        <button
          className="btn primary small"
          disabled={!!scanning}
          onClick={() =>
            scanPlugins(folders)
              .then(() => setEdited(null))
              .catch((e) => onError(errorMessage(e)))
          }
        >
          <Icon name="refresh" size={14} /> {cache?.scanned_at ? 'Volver a escanear' : 'Escanear'}
        </button>
      </div>
      {scanning && (
        <div className="job">
          <span className="small muted ellipsis">
            {scanning.total ? `${scanning.done} de ${scanning.total} · ${scanning.current.split(/[\\/]/).pop()}` : 'Buscando archivos…'}
          </span>
          <span />
          <div className="progress">
            <div style={{ width: `${scanning.total ? (scanning.done / scanning.total) * 100 : 0}%` }} />
          </div>
        </div>
      )}
      {cache?.scanned_at && !scanning && (
        <>
          <p className="small">
            {count('clap')} CLAP · {count('vst3')} VST3 · último escaneo {formatDate(cache.scanned_at)}
          </p>
          {cache.plugins.length > 0 && (
            <ul className="plugin-list small">
              {cache.plugins.map((p) => (
                <li key={`${p.format}:${p.path}:${p.id}`}>
                  <span className="badge">{FORMAT_LABEL[p.format]}</span> {p.name} <span className="muted">· {p.vendor}</span>
                </li>
              ))}
            </ul>
          )}
          {cache.failed.length > 0 && (
            <>
              <button className="link small" onClick={() => setShowFailed(!showFailed)}>
                {cache.failed.length} {cache.failed.length === 1 ? 'archivo no se pudo cargar' : 'archivos no se pudieron cargar'}
              </button>
              {showFailed && (
                <ul className="plugin-list small">
                  {cache.failed.map((f) => (
                    <li key={f.path} title={f.path}>
                      <span className="mono">{f.path.split(/[\\/]/).pop()}</span> <span className="muted">· {f.error}</span>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </>
      )}
    </article>
  )
}

const STATE_LABEL: Record<WorkerStatus['state'], string> = {
  stopped: 'Detenido',
  starting: 'Iniciando…',
  ready: 'Listo',
  crashed: 'Con errores',
}

export function SystemView({ info, worker, onError }: { info: AppInfo | null; worker: WorkerStatus | null; onError: (m: string) => void }) {
  const [sys, setSys] = useState<SystemInfo | null>(null)

  useEffect(() => {
    if (worker?.state !== 'ready') return
    api.worker<SystemInfo>('system.info').then(setSys).catch((e) => onError(errorMessage(e)))
  }, [worker?.state, onError])

  const accel = sys?.torch?.cuda
    ? `CUDA ${sys.torch.cuda_version ?? ''}`
    : sys?.torch
      ? 'PyTorch en CPU'
      : 'PyTorch no instalado (se instalará con el primer modelo que lo necesite)'

  return (
    <section className="system">
      <header className="view-header">
        <div>
          <h1>Sistema</h1>
          <p className="muted">MusicNova {info?.version} · GPL-3.0</p>
        </div>
      </header>

      <div className="card-grid">
        <article className="card">
          <h3>Audio</h3>
          {info?.engine_error ? (
            <p className="error-text">{info.engine_error}</p>
          ) : (
            <dl>
              <dt>Dispositivo</dt>
              <dd>{info?.device ?? '—'}</dd>
              <dt>Frecuencia</dt>
              <dd>{info?.sample_rate ? `${info.sample_rate} Hz` : '—'}</dd>
              <dt>Proyectos</dt>
              <dd className="mono small">{info?.projects_root}</dd>
            </dl>
          )}
        </article>

        <article className="card">
          <div className="model-head">
            <h3>Proceso de IA</h3>
            <span className={`badge ${worker?.state === 'ready' ? 'ok' : worker?.state === 'crashed' ? 'warn' : ''}`}>
              {worker ? STATE_LABEL[worker.state] : '—'}
            </span>
          </div>
          <dl>
            <dt>Python</dt>
            <dd className="mono small">{sys?.python ? `${sys.python} · ${worker?.python}` : worker?.python}</dd>
            <dt>Datos</dt>
            <dd className="mono small">{sys?.data_dir ?? '—'}</dd>
          </dl>
          {worker?.error && <p className="error-text">{worker.error}</p>}
          {worker?.state === 'crashed' && worker.stderr_tail.length > 0 && (
            <pre className="log">{worker.stderr_tail.join('\n')}</pre>
          )}
          <button className="btn ghost" onClick={() => api.workerRestart().catch((e) => onError(errorMessage(e)))}>
            <Icon name="refresh" size={16} /> Reiniciar
          </button>
        </article>

        <article className="card">
          <h3>Hardware</h3>
          <dl>
            <dt>CPU</dt>
            <dd>{sys ? `${sys.cpu_count} hilos · ${sys.machine}` : '—'}</dd>
            <dt>RAM</dt>
            <dd>{formatBytes(sys?.ram_bytes)}</dd>
            <dt>GPU</dt>
            <dd>
              {sys?.gpus.length
                ? sys.gpus.map((g) => `${g.name} (${formatBytes(g.vram_bytes)})`).join(', ')
                : 'No se detectó GPU NVIDIA'}
            </dd>
            <dt>Aceleración</dt>
            <dd>{sys ? accel : '—'}</dd>
            <dt>Sistema</dt>
            <dd className="small">{sys?.platform ?? '—'}</dd>
          </dl>
        </article>

        <PluginsCard onError={onError} />
      </div>
    </section>
  )
}
