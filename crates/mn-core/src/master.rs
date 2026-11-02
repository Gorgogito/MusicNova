//! Masterización automática a una sonoridad objetivo.
//!
//! Sobre la premezcla (pistas con sus efectos, sin el bus máster) se aplican
//! los efectos del máster que haya puesto el usuario y, al final, un limitador
//! de pico real. Se busca la ganancia que deja la sonoridad integrada en el
//! objetivo (p. ej. -14 LUFS) sin superar el techo de pico real (p. ej. -1 dBTP).
//! Si hace falta subir, la ganancia entra en el limitador; si hace falta bajar,
//! se aplica después como ganancia del máster.

use mn_fx::loudness::{loudness_only, measure, LoudnessReport};
use mn_fx::{Chain, EffectParams, EffectSlot, LimiterParams};

/// Identificador del limitador que gestiona la masterización automática.
pub const AUTO_LIMITER_ID: &str = "auto_limiter";
const BLOCK: usize = 512;
/// Tolerancia de la búsqueda (LU) y número máximo de pasadas.
const TOLERANCE: f64 = 0.1;
const MAX_PASSES: usize = 8;
const MAX_BOOST_DB: f64 = 24.0;

#[derive(Clone, Debug, PartialEq)]
pub struct AutoMasterResult {
    /// Ganancia de entrada del limitador y ganancia final del máster (dB).
    pub limiter_input_db: f32,
    pub master_gain_db: f32,
    pub before: LoudnessReport,
    pub after: LoudnessReport,
    /// Pasadas de render que necesitó la búsqueda.
    pub passes: usize,
    /// `false` si no se alcanzó el objetivo (p. ej. haría falta más de +24 dB).
    pub reached: bool,
}

/// Efectos del máster sin el limitador automático.
pub fn user_master_effects(effects: &[EffectSlot]) -> Vec<EffectSlot> {
    effects.iter().filter(|s| s.id != AUTO_LIMITER_ID).cloned().collect()
}

/// Cadena de máster resultante: la del usuario más el limitador automático al final.
pub fn with_auto_limiter(effects: &[EffectSlot], input_db: f32, ceiling_db: f32) -> Vec<EffectSlot> {
    let mut out = user_master_effects(effects);
    out.push(EffectSlot { plugin: None,
        id: AUTO_LIMITER_ID.into(),
        enabled: true,
        params: EffectParams::Limiter(LimiterParams { input_db, ceiling_db, release_ms: 80.0, true_peak: true }),
    });
    out
}

fn process(chain: &mut Chain, data: &mut [f32]) {
    for chunk in data.chunks_mut(BLOCK * 2) {
        chain.process(chunk);
    }
}

/// Busca la ganancia que deja `premaster` en `target_lufs` con techo `ceiling_db` (dBTP).
pub fn auto_master(
    premaster: &[f32],
    sample_rate: u32,
    user_effects: &[EffectSlot],
    target_lufs: f64,
    ceiling_db: f32,
    mut progress: impl FnMut(usize),
) -> AutoMasterResult {
    // Los efectos del usuario no dependen de la ganancia: se aplican una vez.
    let mut pre = premaster.to_vec();
    let user = user_master_effects(user_effects);
    if !user.is_empty() {
        process(&mut Chain::new_offline(&user, sample_rate), &mut pre);
    }
    let before = measure(&pre, sample_rate);

    // `x` > 0 sube la entrada del limitador; `x` < 0 baja la ganancia final.
    let split = |x: f64| if x >= 0.0 { (x.min(MAX_BOOST_DB) as f32, 0.0f32) } else { (0.0f32, x.max(-24.0) as f32) };
    // Un único búfer de trabajo para todas las pasadas.
    let mut out = vec![0.0f32; pre.len()];
    let mut render = |x: f64, full: bool| -> LoudnessReport {
        let (input_db, gain_db) = split(x);
        out.copy_from_slice(&pre);
        let slots = with_auto_limiter(&[], input_db, ceiling_db);
        process(&mut Chain::new_offline(&slots, sample_rate), &mut out);
        if gain_db != 0.0 {
            let g = 10f32.powf(gain_db / 20.0);
            out.iter_mut().for_each(|s| *s *= g);
        }
        if full {
            measure(&out, sample_rate)
        } else {
            // Durante la búsqueda basta la sonoridad: el limitador ya garantiza el techo.
            let (integrated, range) = loudness_only(&out, sample_rate);
            LoudnessReport { integrated, range, true_peak_db: f64::NAN, sample_peak_db: f64::NAN }
        }
    };

    if !before.integrated.is_finite() {
        // Silencio: no hay nada que ajustar.
        return AutoMasterResult { limiter_input_db: 0.0, master_gain_db: 0.0, before, after: before, passes: 0, reached: false };
    }

    // Secante sobre f(x) = LUFS(x) - objetivo, partiendo de la estimación lineal.
    let mut x0 = 0.0;
    let mut r0 = render(x0, false);
    let mut passes = 1;
    progress(passes);
    let mut x1 = (target_lufs - r0.integrated).clamp(-24.0, MAX_BOOST_DB);
    let mut r1 = render(x1, false);
    passes += 1;
    progress(passes);
    while (r1.integrated - target_lufs).abs() > TOLERANCE && passes < MAX_PASSES {
        let (f0, f1) = (r0.integrated - target_lufs, r1.integrated - target_lufs);
        let slope = if (f1 - f0).abs() > 1e-6 { (f1 - f0) / (x1 - x0) } else { 1.0 };
        // La sonoridad crece con la ganancia; con limitación fuerte la pendiente baja.
        let next = (x1 - f1 / slope.max(0.05)).clamp(-24.0, MAX_BOOST_DB);
        if (next - x1).abs() < 1e-3 {
            break;
        }
        (x0, r0) = (x1, r1);
        x1 = next;
        r1 = render(x1, false);
        passes += 1;
        progress(passes);
    }
    let (limiter_input_db, master_gain_db) = split(x1);
    // Medida completa (con pico real) solo del resultado final.
    let r1 = render(x1, true);
    AutoMasterResult {
        limiter_input_db,
        master_gain_db,
        before,
        after: r1,
        passes,
        reached: (r1.integrated - target_lufs).abs() <= TOLERANCE * 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Música sintética con dinámica: acordes con envolvente y golpes de bombo.
    fn music(sr: u32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (sr as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let t = i as f32 / sr as f32;
                let beat = t % 0.5;
                // Bombo: barrido de 150 a 50 Hz con la fase integrada (sin aliasing).
                let phase = 50.0 * beat + 100.0 * (1.0 - (-beat * 40.0).exp()) / 40.0;
                let kick = (-beat * 30.0).exp() * (std::f32::consts::TAU * phase).sin();
                let chord: f32 = [220.0, 277.2, 329.6].iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() / 3.0;
                let s = amp * (0.6 * kick + 0.4 * chord * (0.6 + 0.4 * (t * 0.5).sin()));
                [s, s * 0.95]
            })
            .collect()
    }

    #[test]
    fn reaches_streaming_target_without_exceeding_true_peak() {
        let sr = 44_100;
        let quiet = music(sr, 20.0, 0.15);
        for target in [-14.0, -9.0] {
            let r = auto_master(&quiet, sr, &[], target, -1.0, |_| {});
            println!("objetivo {target}: {:.2} → {:.2} LUFS, TP {:.2} dBTP, entrada +{:.1} dB, {} pasadas", r.before.integrated, r.after.integrated, r.after.true_peak_db, r.limiter_input_db, r.passes);
            assert!(r.reached, "{r:?}");
            assert!((r.after.integrated - target).abs() <= 0.2);
            assert!(r.after.true_peak_db <= -0.95, "TP {}", r.after.true_peak_db);
            assert!(r.passes <= MAX_PASSES);
        }
    }

    #[test]
    fn turns_down_a_loud_mix() {
        let sr = 44_100;
        let loud = music(sr, 20.0, 0.9);
        let r = auto_master(&loud, sr, &[], -23.0, -1.0, |_| {});
        assert!(r.reached, "{r:?}");
        assert_eq!(r.limiter_input_db, 0.0);
        assert!(r.master_gain_db < 0.0);
        assert!((r.after.integrated + 23.0).abs() <= 0.2);
    }

    #[test]
    fn keeps_user_effects_and_replaces_auto_limiter() {
        let user = vec![EffectSlot { plugin: None, id: "eq".into(), enabled: true, params: EffectParams::Eq(Default::default()) }];
        let chain = with_auto_limiter(&with_auto_limiter(&user, 3.0, -1.0), 5.0, -2.0);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].id, "eq");
        let EffectParams::Limiter(l) = chain[1].params else { panic!() };
        assert_eq!((l.input_db, l.ceiling_db, l.true_peak), (5.0, -2.0, true));
    }
}
