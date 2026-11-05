import { useEffect, useState } from 'react'
import { api, errorMessage, type AppInfo, type SystemInfo, type WorkerStatus } from '../lib/api'
import { formatBytes } from '../lib/format'
import { Icon } from '../components/Icon'
import { open } from '@tauri-apps/plugin-dialog'
import { FORMAT_LABEL, scanPlugins, usePlugins } from '../lib/plugins'
import { formatDate } from '../lib/format'
import { LANGS, lang, plural, setLang, tr, type Lang, trBackend } from '../i18n'
import type { Updates } from '../lib/updates'

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
        <h3>{tr('Plugins CLAP y VST3')}</h3>
        {cache?.scanned_at && <span className="badge">{plural(cache.plugins.length, '{n} plugin', '{n} plugins')}</span>}
      </div>
      <p className="small muted">
        {tr('Se buscan en las carpetas estándar y en las que añadas. Cada archivo se examina en un proceso aparte, así que un plugin defectuoso no cierra MusicNova durante el escaneo. Al usarlos, cada plugin corre en su propio proceso: si falla, MusicNova sigue funcionando.')}
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
            <button className="icon-btn small" title={tr('Quitar carpeta')} onClick={() => setEdited(folders.filter((x) => x !== f))}>
              <Icon name="x" size={12} />
            </button>
          </li>
        ))}
      </ul>
      <div className="panel-row">
        <button className="btn ghost small" onClick={addFolder} disabled={!!scanning}>
          <Icon name="folder" size={14} /> {tr('Añadir carpeta')}
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
          <Icon name="refresh" size={14} /> {cache?.scanned_at ? tr('Volver a escanear') : tr('Escanear')}
        </button>
      </div>
      {scanning && (
        <div className="job">
          <span className="small muted ellipsis">
            {scanning.total ? tr('{done} de {total} · {file}', { done: scanning.done, total: scanning.total, file: scanning.current.split(/[\\/]/).pop() ?? '' }) : tr('Buscando archivos…')}
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
            {count('clap')} CLAP · {count('vst3')} VST3 · {tr('último escaneo {date}', { date: formatDate(cache.scanned_at) })}
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
                {plural(cache.failed.length, '{n} archivo no se pudo cargar', '{n} archivos no se pudieron cargar')}
              </button>
              {showFailed && (
                <ul className="plugin-list small">
                  {cache.failed.map((f) => (
                    <li key={f.path} title={f.path}>
                      <span className="mono">{f.path.split(/[\\/]/).pop()}</span> <span className="muted">· {trBackend(f.error)}</span>
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
  stopped: tr('Detenido'),
  starting: tr('Iniciando…'),
  ready: tr('Listo'),
  crashed: tr('Con errores'),
}

export function SystemView({
  info,
  worker,
  updates,
  onError,
}: {
  info: AppInfo | null
  worker: WorkerStatus | null
  updates: Updates
  onError: (m: string) => void
}) {
  const [sys, setSys] = useState<SystemInfo | null>(null)

  useEffect(() => {
    if (worker?.state !== 'ready') return
    api.worker<SystemInfo>('system.info').then(setSys).catch((e) => onError(errorMessage(e)))
  }, [worker?.state, onError])

  const accel = sys?.torch?.cuda
    ? `CUDA ${sys.torch.cuda_version ?? ''}`
    : sys?.torch
      ? tr('PyTorch en CPU')
      : tr('PyTorch no instalado (se instalará con el primer modelo que lo necesite)')

  return (
    <section className="system">
      <header className="view-header">
        <div>
          <h1>{tr('Sistema')}</h1>
          <p className="muted">MusicNova {info?.version} · GPL-3.0</p>
        </div>
        <label className="field">
          <span className="small muted">Idioma · Language</span>
          <select value={lang()} onChange={(e) => setLang(e.target.value as Lang)}>
            {LANGS.map((l) => (
              <option key={l.id} value={l.id}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
      </header>

      <div className="card-grid">
        <UpdatesCard updates={updates} />
        <article className="card">
          <h3>{tr('Audio')}</h3>
          {info?.engine_error ? (
            <p className="error-text">{info.engine_error}</p>
          ) : (
            <dl>
              <dt>{tr('Dispositivo')}</dt>
              <dd>{info?.device ?? '—'}</dd>
              <dt>{tr('Frecuencia')}</dt>
              <dd>{info?.sample_rate ? `${info.sample_rate} Hz` : '—'}</dd>
              <dt>{tr('Proyectos')}</dt>
              <dd className="mono small">{info?.projects_root}</dd>
            </dl>
          )}
        </article>

        <article className="card">
          <div className="model-head">
            <h3>{tr('Proceso de IA')}</h3>
            <span className={`badge ${worker?.state === 'ready' ? 'ok' : worker?.state === 'crashed' ? 'warn' : ''}`}>
              {worker ? STATE_LABEL[worker.state] : '—'}
            </span>
          </div>
          <dl>
            <dt>{tr('Python')}</dt>
            <dd className="mono small">{sys?.python ? `${sys.python} · ${worker?.python}` : worker?.python}</dd>
            <dt>{tr('Datos')}</dt>
            <dd className="mono small">{sys?.data_dir ?? '—'}</dd>
          </dl>
          {worker?.error && <p className="error-text">{worker.error}</p>}
          {worker?.state === 'crashed' && worker.stderr_tail.length > 0 && (
            <pre className="log">{worker.stderr_tail.join('\n')}</pre>
          )}
          <button className="btn ghost" onClick={() => api.workerRestart().catch((e) => onError(errorMessage(e)))}>
            <Icon name="refresh" size={16} /> {tr('Reiniciar')}
          </button>
        </article>

        <article className="card">
          <h3>{tr('Hardware')}</h3>
          {!sys && worker?.state === 'ready' && <span className="small muted">{tr('Comprobando el equipo…')}</span>}
          <dl>
            <dt>CPU</dt>
            <dd>{sys ? `${sys.cpu_count} hilos · ${sys.machine}` : '—'}</dd>
            <dt>RAM</dt>
            <dd>{formatBytes(sys?.ram_bytes)}</dd>
            <dt>GPU</dt>
            <dd>
              {!sys
                ? '—'
                : sys.gpus.length
                  ? sys.gpus.map((g) => `${g.name} (${formatBytes(g.vram_bytes)})`).join(', ')
                  : tr('No se detectó GPU NVIDIA')}
            </dd>
            <dt>{tr('Aceleración')}</dt>
            <dd>{sys ? accel : '—'}</dd>
            <dt>{tr('Sistema')}</dt>
            <dd className="small">{sys?.platform ?? '—'}</dd>
          </dl>
        </article>

        <PluginsCard onError={onError} />
      </div>
    </section>
  )
}

/** Versión instalada y búsqueda de versiones nuevas (GitHub Releases). */
function UpdatesCard({ updates }: { updates: Updates }) {
  const s = updates.state
  const busy = s.kind === 'checking' || s.kind === 'installing'
  return (
    <article className="card">
      <h3>{tr('Actualizaciones')}</h3>
      <p className="small muted">
        {s.kind === 'checking'
          ? tr('Buscando versiones nuevas…')
          : s.kind === 'current'
            ? tr('Tienes la última versión.')
            : s.kind === 'available'
              ? tr('MusicNova {version} está disponible (tienes la {current}).', { version: s.info.version, current: s.info.current })
              : s.kind === 'installing'
                ? tr('Instalando MusicNova {version}…', { version: s.info.version })
                : s.kind === 'error'
                  ? s.message
                  : tr('Las versiones nuevas se publican en GitHub y se instalan sin perder tus proyectos ni los modelos descargados.')}
      </p>
      {s.kind === 'available' && s.info.notes && <pre className="update-notes small">{s.info.notes}</pre>}
      <div className="panel-row">
        {s.kind === 'available' ? (
          <button className="btn primary" disabled={busy} onClick={updates.install}>
            <Icon name="download" size={16} /> {tr('Instalar y reiniciar')}
          </button>
        ) : (
          <button className="btn" disabled={busy} onClick={() => updates.check()}>
            <Icon name="refresh" size={16} /> {tr('Buscar actualizaciones')}
          </button>
        )}
        <label className="check small">
          <input type="checkbox" checked={updates.auto} onChange={(e) => updates.setAuto(e.target.checked)} />
          {tr('Buscar al arrancar')}
        </label>
      </div>
    </article>
  )
}
