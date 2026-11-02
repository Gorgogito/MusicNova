//! Publicación sin bloqueos de pares de valores entre hilos (seqlock) y
//! conversión de los instantes de cpal a nanosegundos.

use std::sync::atomic::{fence, AtomicU64, Ordering};

/// Par de `u64` con un único escritor y lectores que nunca ven una mezcla de
/// dos escrituras. El escritor nunca espera (apto para el hilo de audio).
pub(crate) struct Pair {
    seq: AtomicU64,
    a: AtomicU64,
    b: AtomicU64,
}

impl Pair {
    pub fn new() -> Self {
        Self { seq: AtomicU64::new(0), a: AtomicU64::new(0), b: AtomicU64::new(0) }
    }

    /// Solo debe llamarlo un hilo.
    pub fn store(&self, a: u64, b: u64) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.a.store(a, Ordering::Relaxed);
        self.b.store(b, Ordering::Relaxed);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// Devuelve `(versión, a, b)`; la versión es 0 si nunca se escribió y
    /// cambia con cada escritura.
    pub fn load(&self) -> (u64, u64, u64) {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let a = self.a.load(Ordering::Relaxed);
            let b = self.b.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return (s1 / 2, a, b);
            }
        }
    }
}

/// Instante de cpal en nanosegundos desde el origen de su reloj (QPC en WASAPI;
/// la entrada y la salida comparten reloj).
pub(crate) fn instant_nanos(i: &cpal::StreamInstant) -> u64 {
    i.duration_since(&cpal::StreamInstant::new(0, 0)).map(|d| d.as_nanos() as u64).unwrap_or(0)
}

/// Instante (ns) del frame `frame` de un flujo a `rate` Hz, sabiendo que el
/// frame `anchor_frame` ocurre en `anchor_nanos`.
pub(crate) fn frame_instant(anchor_frame: u64, anchor_nanos: u64, frame: u64, rate: u32) -> i128 {
    anchor_nanos as i128 + (frame as i128 - anchor_frame as i128) * 1_000_000_000 / rate as i128
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn pair_is_consistent_under_contention() {
        let p = Arc::new(Pair::new());
        assert_eq!(p.load(), (0, 0, 0));
        let w = p.clone();
        let writer = std::thread::spawn(move || {
            for i in 1..=200_000u64 {
                w.store(i, i * 3);
            }
        });
        let mut last = 0;
        while last < 200_000 {
            let (_, a, b) = p.load();
            assert_eq!(b, a * 3);
            assert!(a >= last);
            last = a;
        }
        writer.join().unwrap();
        assert_eq!(p.load().0, 200_000);
    }

    #[test]
    fn frame_instants() {
        assert_eq!(frame_instant(100, 1_000_000_000, 148, 48_000), 1_001_000_000);
        assert_eq!(frame_instant(100, 1_000_000_000, 52, 48_000), 999_000_000);
    }
}
