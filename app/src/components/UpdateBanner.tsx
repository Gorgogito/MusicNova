import { tr } from '../i18n'
import type { Updates } from '../lib/updates'
import { Icon } from './Icon'

/** Aviso de versión nueva (arriba del contenido) con instalación y progreso. */
export function UpdateBanner({ updates }: { updates: Updates }) {
  const s = updates.state
  if (s.kind === 'installing') {
    const pct = s.total ? Math.round((s.downloaded / s.total) * 100) : null
    return (
      <div className="update-banner">
        <Icon name="download" size={16} />
        <span>
          {pct === null
            ? tr('Descargando MusicNova {version}…', { version: s.info.version })
            : tr('Descargando MusicNova {version}… {pct}%', { version: s.info.version, pct })}{' '}
          {tr('Al terminar se instalará y la aplicación se reiniciará.')}
        </span>
        <div className="progress">
          <div style={{ width: `${pct ?? 5}%` }} />
        </div>
      </div>
    )
  }
  if (s.kind !== 'available' || updates.dismissed) return null
  return (
    <div className="update-banner">
      <Icon name="download" size={16} />
      <span>
        {tr('MusicNova {version} está disponible (tienes la {current}).', { version: s.info.version, current: s.info.current })}
      </span>
      <button className="btn primary small" onClick={updates.install}>
        {tr('Instalar y reiniciar')}
      </button>
      <button className="btn ghost small" onClick={updates.dismiss}>
        {tr('Más tarde')}
      </button>
    </div>
  )
}
