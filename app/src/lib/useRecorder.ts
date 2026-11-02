import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, type InputChannels, type InputDeviceInfo, type InputInfo, type LatencyReport } from './api'

/** Ajustes de grabación guardados en este equipo (no en el proyecto). */
export interface RecorderPrefs {
  device: string | null
  channels: InputChannels
  latencyMs: number
  monitorGainDb: number
}

const PREFS_KEY = 'musicnova.recorder'
const DEFAULTS: RecorderPrefs = { device: null, channels: { mode: 'mono', first: 0 }, latencyMs: 0, monitorGainDb: 0 }

function loadPrefs(): RecorderPrefs {
  try {
    const raw = localStorage.getItem(PREFS_KEY)
    if (!raw) return DEFAULTS
    const p = JSON.parse(raw) as Partial<RecorderPrefs>
    return {
      device: typeof p.device === 'string' ? p.device : null,
      channels:
        p.channels && (p.channels.mode === 'mono' || p.channels.mode === 'stereo') && Number.isInteger(p.channels.first)
          ? p.channels
          : DEFAULTS.channels,
      latencyMs: Number.isFinite(p.latencyMs) ? Number(p.latencyMs) : 0,
      monitorGainDb: Number.isFinite(p.monitorGainDb) ? Number(p.monitorGainDb) : 0,
    }
  } catch {
    return DEFAULTS
  }
}

function savePrefs(p: RecorderPrefs) {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(p))
  } catch {
    // Sin almacenamiento local: los ajustes duran hasta cerrar la app.
  }
}

export interface Recorder {
  devices: InputDeviceInfo[]
  input: InputInfo | null
  prefs: RecorderPrefs
  monitor: boolean
  busy: string | null
  /** Última calibración de esta sesión (la latencia cambia al reiniciar la app). */
  calibration: LatencyReport | null
  /** Posición (s) en la que se pulsó grabar; `null` si no se está grabando. */
  startedAt: number | null
  refreshDevices: () => void
  setPrefs: (patch: Partial<RecorderPrefs>) => void
  open: () => Promise<void>
  close: () => void
  setMonitor: (enabled: boolean, gainDb?: number) => void
  calibrate: () => Promise<void>
  start: (position: number) => void
  stop: () => void
  /** Avisa de que la toma terminó (evento del motor). */
  finished: () => void
}

export function useRecorder(onError: (m: string) => void): Recorder {
  const [devices, setDevices] = useState<InputDeviceInfo[]>([])
  const [input, setInput] = useState<InputInfo | null>(null)
  const [prefs, setPrefsState] = useState<RecorderPrefs>(loadPrefs)
  const [monitor, setMonitorState] = useState(false)
  const [busy, setBusy] = useState<string | null>(null)
  const [calibration, setCalibration] = useState<LatencyReport | null>(null)
  const [startedAt, setStartedAt] = useState<number | null>(null)
  const prefsRef = useRef(prefs)

  const setPrefs = useCallback((patch: Partial<RecorderPrefs>) => {
    const next = { ...prefsRef.current, ...patch }
    prefsRef.current = next
    setPrefsState(next)
    savePrefs(next)
  }, [])

  const refreshDevices = useCallback(() => {
    api.inputDevices().then(setDevices).catch((e) => onError(errorMessage(e)))
  }, [onError])

  useEffect(refreshDevices, [refreshDevices])

  const open = useCallback(async () => {
    setBusy('Abriendo la entrada…')
    try {
      const p = prefsRef.current
      const info = await api.inputOpen(p.device, p.channels)
      setInput(info)
    } catch (e) {
      onError(errorMessage(e))
    } finally {
      setBusy(null)
    }
  }, [onError])

  const close = useCallback(() => {
    api
      .inputClose()
      .then(() => setInput(null))
      .catch((e) => onError(errorMessage(e)))
  }, [onError])

  const setMonitor = useCallback(
    (enabled: boolean, gainDb?: number) => {
      const gain = gainDb ?? prefsRef.current.monitorGainDb
      if (gainDb !== undefined) setPrefs({ monitorGainDb: gainDb })
      setMonitorState(enabled)
      api.inputMonitor(enabled, gain).catch((e) => onError(errorMessage(e)))
    },
    [onError, setPrefs],
  )

  const calibrate = useCallback(async () => {
    setBusy('Midiendo la latencia…')
    try {
      const r = await api.recordCalibrate()
      setCalibration(r)
      setPrefs({ latencyMs: Math.round(r.latency_ms * 100) / 100 })
    } catch (e) {
      onError(errorMessage(e))
    } finally {
      setBusy(null)
    }
  }, [onError, setPrefs])

  const start = useCallback(
    (position: number) => {
      setStartedAt(position)
      api.recordStart(prefsRef.current.latencyMs).catch((e) => {
        setStartedAt(null)
        onError(errorMessage(e))
      })
    },
    [onError],
  )

  const stop = useCallback(() => {
    api.recordStop().catch((e) => onError(errorMessage(e)))
  }, [onError])

  const finished = useCallback(() => setStartedAt(null), [])

  return {
    devices,
    input,
    prefs,
    monitor,
    busy,
    calibration,
    startedAt,
    refreshDevices,
    setPrefs,
    open,
    close,
    setMonitor,
    calibrate,
    start,
    stop,
    finished,
  }
}
