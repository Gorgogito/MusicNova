//! Cambio de tempo y tono de alta calidad con Signalsmith Stretch.
//!
//! Código C++ de Signalsmith Audio (MIT) y envoltura C de Colin Marc (MIT),
//! incluidos en `vendor/`. Tras crear la instancia, `process` no reserva memoria,
//! por lo que puede usarse en el hilo de audio.

use std::ffi::c_int;

#[repr(C)]
struct RawStretch {
    _private: [u8; 0],
}

extern "C" {
    fn signalsmith_stretch_create_preset_default(channel_count: c_int, sample_rate: f32) -> *mut RawStretch;
    fn signalsmith_stretch_destroy(handle: *mut RawStretch);
    fn signalsmith_stretch_reset(handle: *mut RawStretch);
    fn signalsmith_stretch_input_latency(handle: *mut RawStretch) -> usize;
    fn signalsmith_stretch_output_latency(handle: *mut RawStretch) -> usize;
    fn signalsmith_stretch_set_transpose_factor_semitones(handle: *mut RawStretch, semitones: f32, tonality_limit: f32);
    fn signalsmith_stretch_process(
        handle: *mut RawStretch,
        input: *mut f32,
        input_length: usize,
        output: *mut f32,
        output_length: usize,
    );
    fn signalsmith_stretch_seek(handle: *mut RawStretch, input: *mut f32, input_length: usize, playback_rate: f64);
    fn signalsmith_stretch_set_formant_factor(handle: *mut RawStretch, multiplier: f32, compensate_pitch: c_int);
}

/// Instancia estéreo de Signalsmith Stretch.
pub struct Stretch {
    raw: *mut RawStretch,
}

// La instancia es propiedad exclusiva de un hilo a la vez.
unsafe impl Send for Stretch {}

const CHANNELS: usize = 2;

impl Stretch {
    pub fn new(sample_rate: u32) -> Self {
        let raw = unsafe { signalsmith_stretch_create_preset_default(CHANNELS as c_int, sample_rate as f32) };
        assert!(!raw.is_null(), "no se pudo crear Signalsmith Stretch");
        Self { raw }
    }

    pub fn reset(&mut self) {
        unsafe { signalsmith_stretch_reset(self.raw) }
    }

    pub fn input_latency(&self) -> usize {
        unsafe { signalsmith_stretch_input_latency(self.raw) }
    }

    pub fn output_latency(&self) -> usize {
        unsafe { signalsmith_stretch_output_latency(self.raw) }
    }

    /// Transposición en semitonos (sin límite de tonalidad).
    pub fn set_transpose_semitones(&mut self, semitones: f32) {
        unsafe { signalsmith_stretch_set_transpose_factor_semitones(self.raw, semitones, 0.0) }
    }

    /// Conserva los formantes al transponer (el timbre de una voz no cambia
    /// aunque cambie su altura).
    pub fn set_preserve_formants(&mut self, on: bool) {
        unsafe { signalsmith_stretch_set_formant_factor(self.raw, 1.0, on as c_int) }
    }

    /// Procesa audio estéreo entrelazado. La razón entre las longitudes de
    /// entrada y salida determina el cambio de tempo.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len() % CHANNELS, 0);
        debug_assert_eq!(output.len() % CHANNELS, 0);
        // La API C no modifica la entrada aunque no la declare `const`.
        unsafe {
            signalsmith_stretch_process(
                self.raw,
                input.as_ptr() as *mut f32,
                input.len() / CHANNELS,
                output.as_mut_ptr(),
                output.len() / CHANNELS,
            )
        }
    }

    /// Prepara la instancia tras un salto: alimenta audio previo sin producir salida.
    pub fn seek(&mut self, input: &[f32], playback_rate: f64) {
        unsafe { signalsmith_stretch_seek(self.raw, input.as_ptr() as *mut f32, input.len() / CHANNELS, playback_rate) }
    }
}

impl Drop for Stretch {
    fn drop(&mut self) {
        unsafe { signalsmith_stretch_destroy(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin() * 0.5;
                [s, s]
            })
            .collect()
    }

    fn freq(data: &[f32], rate: u32) -> f32 {
        let left: Vec<f32> = data.chunks_exact(2).map(|f| f[0]).collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        crossings as f32 / 2.0 / (left.len() as f32 / rate as f32)
    }

    #[test]
    fn transposes_pitch_without_changing_length() {
        let rate = 48_000;
        let input = sine(rate, 440.0, rate as usize * 2);
        let mut st = Stretch::new(rate);
        st.set_transpose_semitones(12.0);
        let mut out = vec![0.0; input.len()];
        st.process(&input, &mut out);
        let lat = st.output_latency() + st.input_latency();
        let f = freq(&out[(lat + 4800) * 2..], rate);
        assert!((f - 880.0).abs() < 15.0, "frecuencia {f}");
    }

    #[test]
    fn slows_down_keeping_pitch() {
        let rate = 48_000;
        let input = sine(rate, 440.0, rate as usize);
        let mut st = Stretch::new(rate);
        // La mitad de velocidad: el doble de salida para la misma entrada.
        let mut out = vec![0.0; input.len() * 2];
        st.process(&input, &mut out);
        let lat = st.output_latency() + st.input_latency();
        let f = freq(&out[(lat + 4800) * 2..out.len() - 9600], rate);
        assert!((f - 440.0).abs() < 10.0, "frecuencia {f}");
    }
}
