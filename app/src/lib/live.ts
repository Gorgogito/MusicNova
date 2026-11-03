// Tocar en vivo: teclado MIDI o del ordenador sobre la pista de instrumento armada.
import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type LiveStatus, type Note } from './api'

/** Teclas del ordenador → semitonos sobre el Do de la octava actual (como en los DAW). */
const KEYS: Record<string, number> = {
  KeyA: 0, KeyW: 1, KeyS: 2, KeyE: 3, KeyD: 4, KeyF: 5, KeyT: 6, KeyG: 7, KeyY: 8, KeyH: 9,
  KeyU: 10, KeyJ: 11, KeyK: 12, KeyO: 13, KeyL: 14, KeyP: 15, Semicolon: 16,
}
const NAMES = ['Do', 'Do♯', 'Re', 'Re♯', 'Mi', 'Fa', 'Fa♯', 'Sol', 'Sol♯', 'La', 'La♯', 'Si']
export const noteName = (pitch: number) => `${NAMES[pitch % 12]}${Math.floor(pitch / 12) - 1}`

export interface LivePlay {
  status: LiveStatus | null
  refresh: () => void
  openDevice: (index: number | null) => void
  arm: (trackId: string | null) => Promise<void>
  recording: boolean
  setRecording: (on: boolean) => void
  keyboard: boolean
  setKeyboard: (on: boolean) => void
  octave: number
  setOctave: (o: number) => void
  velocity: number
  setVelocity: (v: number) => void
  /** Notas que suenan ahora desde el teclado del ordenador. */
  held: number[]
  panic: () => void
}

const DEVICE_KEY = 'musicnova.midiDevice'

export function useLivePlay(opts: {
  playing: boolean
  projectKey: unknown
  onRecorded: (trackId: string, notes: Note[]) => void
  onEditorClosed: (trackId: string) => void
  onError: (m: string) => void
}): LivePlay {
  const { playing, projectKey, onRecorded, onEditorClosed, onError } = opts
  const [status, setStatus] = useState<LiveStatus | null>(null)
  const [recording, setRec] = useState(false)
  const [keyboard, setKeyboard] = useState(false)
  const [octave, setOctave] = useState(4)
  const [velocity, setVelocity] = useState(100)
  const [held, setHeld] = useState<number[]>([])
  const down = useRef(new Map<string, number>())
  const armed = status?.armed ?? null
  const cb = useRef({ onRecorded, onEditorClosed, onError })
  cb.current = { onRecorded, onEditorClosed, onError }

  const fail = (e: unknown) => cb.current.onError(errorMessage(e))
  const refresh = useCallback(() => {
    api.liveStatus().then(setStatus).catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Al abrir la app se vuelve a abrir el último teclado MIDI que estaba conectado.
  useEffect(() => {
    api
      .liveStatus()
      .then((s) => {
        setStatus(s)
        let saved: string | null = null
        try {
          saved = localStorage.getItem(DEVICE_KEY)
        } catch {
          /* sin almacenamiento */
        }
        const i = saved ? s.devices.indexOf(saved) : -1
        if (i >= 0 && !s.open) api.midiOpen(i).then(setStatus).catch(fail)
      })
      .catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  // Recargar el proyecto desarma la pista.
  useEffect(refresh, [projectKey, refresh])

  const openDevice = (index: number | null) => {
    api
      .midiOpen(index)
      .then((s) => {
        setStatus(s)
        try {
          if (s.open) localStorage.setItem(DEVICE_KEY, s.open)
          else localStorage.removeItem(DEVICE_KEY)
        } catch {
          /* sin almacenamiento */
        }
      })
      .catch(fail)
  }

  const arm = async (trackId: string | null) => {
    try {
      setStatus(await api.liveArm(trackId))
    } catch (e) {
      fail(e)
    }
  }

  const take = useCallback(() => {
    const trackId = armed
    api
      .liveTake()
      .then((notes) => {
        if (trackId && notes.length) cb.current.onRecorded(trackId, notes)
      })
      .catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [armed])

  const setRecording = (on: boolean) => {
    api
      .liveRecord(on)
      .then(() => {
        setRec(on)
        if (!on) take()
      })
      .catch(fail)
  }

  // Cada vez que se para el transporte grabando, lo tocado pasa a la pista (una toma).
  const wasPlaying = useRef(playing)
  useEffect(() => {
    if (wasPlaying.current && !playing && recording) take()
    wasPlaying.current = playing
  }, [playing, recording, take])

  const panic = useCallback(() => {
    down.current.clear()
    setHeld([])
    api.livePanic().catch(() => {})
  }, [])

  // Teclado del ordenador como piano (solo con una pista armada).
  useEffect(() => {
    if (!keyboard || !armed) return
    const pressed = down.current
    const typing = (e: KeyboardEvent) => !!(e.target as HTMLElement).closest('input[type="text"], input[type="number"], textarea, select')
    const onDown = (e: KeyboardEvent) => {
      if (typing(e) || e.ctrlKey || e.metaKey || e.altKey) return
      if (e.code === 'KeyZ' || e.code === 'KeyX') {
        e.preventDefault()
        e.stopImmediatePropagation()
        if (!e.repeat) setOctave((o) => Math.max(0, Math.min(8, o + (e.code === 'KeyX' ? 1 : -1))))
        return
      }
      const semi = KEYS[e.code]
      if (semi === undefined) return
      // Estas teclas son del piano: no llegan a los atajos de la aplicación.
      e.preventDefault()
      e.stopImmediatePropagation()
      if (e.repeat || down.current.has(e.code)) return
      const pitch = Math.min(127, 12 * (octave + 1) + semi)
      down.current.set(e.code, pitch)
      setHeld([...down.current.values()])
      api.liveNote(pitch, velocity, true).catch(fail)
    }
    const onUp = (e: KeyboardEvent) => {
      const pitch = down.current.get(e.code)
      if (pitch === undefined) return
      e.preventDefault()
      e.stopImmediatePropagation()
      down.current.delete(e.code)
      setHeld([...down.current.values()])
      api.liveNote(pitch, 0, false).catch(fail)
    }
    window.addEventListener('keydown', onDown, true)
    window.addEventListener('keyup', onUp, true)
    window.addEventListener('blur', panic)
    return () => {
      window.removeEventListener('keydown', onDown, true)
      window.removeEventListener('keyup', onUp, true)
      window.removeEventListener('blur', panic)
      if (pressed.size) panic()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [keyboard, armed, octave, velocity, panic])

  // Al cerrar la interfaz del plugin de la pista armada, su sonido cambia: se regenera.
  useEffect(() => {
    const un = events.pluginEditorClosed((slot) => {
      if (slot.startsWith('inst:')) cb.current.onEditorClosed(slot.slice(5))
    })
    return () => {
      un.then((f) => f())
    }
  }, [])

  return {
    status,
    refresh,
    openDevice,
    arm,
    recording,
    setRecording,
    keyboard,
    setKeyboard,
    octave,
    setOctave,
    velocity,
    setVelocity,
    held,
    panic,
  }
}
