// Actualizaciones: comprobación al arrancar (si está activada) y a petición,
// descarga con progreso e instalación (la app se reinicia sola al terminar).
import { useCallback, useEffect, useState } from 'react'
import { api, errorMessage, events, type UpdateInfo } from './api'

const AUTO_KEY = 'musicnova.updates.auto'
/** Espera tras arrancar antes de comprobar (que no compita con la carga inicial). */
const STARTUP_DELAY_MS = 8000

export type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current'; at: Date }
  | { kind: 'available'; info: UpdateInfo }
  | { kind: 'installing'; info: UpdateInfo; downloaded: number; total: number | null }
  | { kind: 'error'; message: string }

function readAuto(): boolean {
  try {
    return localStorage.getItem(AUTO_KEY) !== '0'
  } catch {
    return true
  }
}

export function useUpdates() {
  const [state, setState] = useState<UpdateState>({ kind: 'idle' })
  const [auto, setAutoState] = useState(readAuto)
  const [dismissed, setDismissed] = useState(false)

  const check = useCallback(async (quiet = false) => {
    setState({ kind: 'checking' })
    try {
      const info = await api.updateCheck()
      setState(info ? { kind: 'available', info } : { kind: 'current', at: new Date() })
      setDismissed(false)
    } catch (e) {
      // Al arrancar, sin conexión o sin versiones publicadas no se molesta al usuario.
      setState(quiet ? { kind: 'idle' } : { kind: 'error', message: errorMessage(e) })
    }
  }, [])

  useEffect(() => {
    if (!auto) return
    const id = window.setTimeout(() => check(true), STARTUP_DELAY_MS)
    return () => window.clearTimeout(id)
    // Solo al arrancar.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const install = useCallback(async () => {
    if (state.kind !== 'available') return
    const info = state.info
    setState({ kind: 'installing', info, downloaded: 0, total: null })
    const un = await events.updateProgress((p) => setState({ kind: 'installing', info, downloaded: p.downloaded, total: p.total }))
    try {
      await api.updateInstall() // si todo va bien, la app se reinicia aquí
    } catch (e) {
      setState({ kind: 'error', message: errorMessage(e) })
    } finally {
      un()
    }
  }, [state])

  const setAuto = (on: boolean) => {
    setAutoState(on)
    try {
      localStorage.setItem(AUTO_KEY, on ? '1' : '0')
    } catch {
      /* sin almacenamiento local */
    }
  }

  return { state, check, install, auto, setAuto, dismissed, dismiss: () => setDismissed(true) }
}

export type Updates = ReturnType<typeof useUpdates>
