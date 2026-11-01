import { useCallback, useState } from 'react'

/** Preferencia de interfaz guardada en este equipo (no en el proyecto). */
export function usePref<T extends string>(key: string, fallback: T, allowed: readonly T[]): [T, (v: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const v = localStorage.getItem(`musicnova.${key}`) as T | null
      return v && allowed.includes(v) ? v : fallback
    } catch {
      return fallback
    }
  })
  const set = useCallback(
    (v: T) => {
      setValue(v)
      try {
        localStorage.setItem(`musicnova.${key}`, v)
      } catch {
        // Sin almacenamiento local: la preferencia dura hasta cerrar la app.
      }
    },
    [key],
  )
  return [value, set]
}
