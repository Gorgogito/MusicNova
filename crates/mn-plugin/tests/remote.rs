//! Plugins en su propio proceso: el mismo plugin de prueba que en `host.rs`,
//! pero cargado a través del anfitrión `mn-plugin-host`. Incluye un plugin que
//! se cae y se cuelga a propósito (`test-crash`) para comprobar que la
//! aplicación sigue en pie. Archivo aparte: el aislamiento es global al proceso.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mn_fx::{Chain, EffectParams, EffectSlot, PluginRef, Processor};
use mn_plugin::{instance, retain_instances, scan_file, sync_states, Instance, PluginProcessor};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

struct Fixture {
    clap: PathBuf,
    vst3: PathBuf,
}

/// Avisos recibidos: (efecto, mensaje) de plugins caídos y ventanas cerradas.
static FAILED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static CLOSED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let status = Command::new(env!("CARGO")).args(["build", "-p", "mn-testplugin"]).current_dir(root).status().expect("cargo build");
        assert!(status.success(), "no se pudo compilar el plugin de prueba");
        let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
        let dll = target.join("debug").join("mn_testplugin.dll");
        let dir = std::env::temp_dir().join(format!("mn_plugins_remote_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = Fixture { clap: dir.join("MusicNovaTest.clap"), vst3: dir.join("MusicNovaTest.vst3") };
        std::fs::copy(&dll, &f.clap).unwrap();
        std::fs::copy(&dll, &f.vst3).unwrap();
        mn_plugin::install();
        mn_plugin::set_host_command(PathBuf::from(env!("CARGO_BIN_EXE_mn-plugin-host")), vec![mn_plugin::HOST_FLAG.into()]);
        mn_plugin::on_plugin_failed(|slot, msg| FAILED.lock().unwrap().push((slot.into(), msg.into())));
        mn_plugin::on_editor_closed(|slot| CLOSED.lock().unwrap().push(slot.into()));
        f
    })
}

fn plugin(path: &Path, id: &str) -> PluginRef {
    scan_file(path).unwrap().into_iter().find(|d| d.id == id || id.is_empty()).expect("plugin en el escaneo").to_ref()
}

fn slot(id: &str, r: &PluginRef) -> EffectSlot {
    EffectSlot { sidechain: None, id: id.into(), enabled: true, params: EffectParams::Plugin, plugin: Some(r.clone()) }
}

/// Procesa un bloque de 0,5 constante y devuelve la primera muestra izquierda.
fn run(p: &mut dyn Processor) -> f32 {
    let mut buf = vec![0.5f32; 2 * 300];
    p.process(&mut buf);
    assert!(buf.chunks_exact(2).all(|f| f[0] == f[1]), "estéreo coherente");
    buf[0]
}

fn run_chain(c: &mut Chain) -> f32 {
    let mut buf = vec![0.5f32; 2 * 300];
    c.process(&mut buf);
    buf[0]
}

fn wait_until(limit: Duration, mut ok: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + limit;
    while Instant::now() < until {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ok()
}

const MINUS_6_DB: f32 = 0.5 * 0.501_187;

#[test]
fn effects_run_in_another_process_like_in_this_one() {
    let f = fixture();
    let r = plugin(&f.clap, "org.musicnova.test-gain");
    let inst = Instance::load_with(&r, 48_000, true).unwrap();
    assert!(inst.isolated(), "con anfitrión configurado, el plugin va en su propio proceso");
    let mut p = PluginProcessor::new(inst.clone());
    assert_eq!(run(&mut p), 0.5);
    assert_eq!(inst.params().len(), 2);
    inst.set_param(0, -6.0);
    assert!((run(&mut p) - MINUS_6_DB).abs() < 1e-5);
    assert!(wait_until(Duration::from_secs(2), || inst.params()[0].value == -6.0), "el plugin refleja el valor");
    assert_eq!(inst.param_from_text(0, "-12 dB"), Some(-12.0), "el texto se convierte en el otro proceso");
    assert_eq!(inst.param_from_text(0, "?"), None);

    // El estado viaja por el proceso: una instancia nueva arranca a -6 dB.
    let state = inst.state_base64().expect("estado");
    let again = Instance::load(&PluginRef { state: Some(state), ..r.clone() }, 48_000).unwrap();
    assert!((run(&mut PluginProcessor::new(again)) - MINUS_6_DB).abs() < 1e-5);

    // Latencia declarada y VST3.
    let delay = Instance::load(&plugin(&f.clap, "org.musicnova.test-delay"), 48_000).unwrap();
    assert_eq!(delay.latency, 1000);
    let v = Instance::load(&plugin(&f.vst3, ""), 48_000).unwrap();
    assert!(v.isolated());
    let mut pv = PluginProcessor::new(v.clone());
    assert_eq!(run(&mut pv), 0.5);
    assert!(!v.params().is_empty());

    // Coste del viaje de ida y vuelta por bloque (bloques de 256 frames).
    let mut buf = vec![0.25f32; 2 * 256];
    let t = Instant::now();
    for _ in 0..2000 {
        p.process(&mut buf);
    }
    let per_block = t.elapsed().as_secs_f64() / 2000.0 * 1e6;
    eprintln!("ida y vuelta por bloque: {per_block:.1} µs (256 frames a 48 kHz = 5333 µs)");
    assert!(per_block < 1000.0, "{per_block} µs por bloque");
}

#[test]
fn instruments_render_in_another_process() {
    let f = fixture();
    let rate = 48_000;
    let notes = [(0.1, 0.6, 69u8, 127u8)];
    let audio = mn_plugin::render_instrument(&plugin(&f.clap, "org.musicnova.test-synth"), &notes, rate, 0.8, 0.1).unwrap();
    let left: Vec<f32> = audio.iter().step_by(2).copied().collect();
    let at = |t: f64| (t * rate as f64) as usize;
    assert!(left[..at(0.099)].iter().all(|&s| s == 0.0));
    let seg = &left[at(0.12)..at(0.58)];
    let hz = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / 0.46;
    assert!((hz - 440.0).abs() < 5.0, "{hz}");
}

#[test]
fn a_crashing_plugin_only_takes_down_its_own_process() {
    let f = fixture();
    let r = plugin(&f.clap, "org.musicnova.test-crash");
    let s = slot("slot-crash", &r);
    let mut chain = Chain::new(std::slice::from_ref(&s), 48_000);
    let inst = instance("slot-crash").expect("instancia en vivo");
    inst.set_param(0, -6.0);
    assert!((run_chain(&mut chain) - MINUS_6_DB).abs() < 1e-5, "procesa con normalidad");
    let mut slots = vec![s.clone()];
    assert!(sync_states(&mut slots), "el estado (-6 dB) queda en el proyecto");

    // +12 dB: el plugin aborta al procesar. El bloque pasa sin procesar y se avisa.
    inst.set_param(0, 12.0);
    let t = Instant::now();
    assert_eq!(run_chain(&mut chain), 0.5, "el audio pasa intacto");
    // Como mucho el límite de un bloque en tiempo real (250 ms) y algo de margen
    // para cuando Windows tarda en dar el proceso por terminado (equipo cargado).
    assert!(t.elapsed() < Duration::from_millis(600), "no se espera al plugin caído ({:?})", t.elapsed());
    assert!(wait_until(Duration::from_secs(2), || inst.failure().is_some()));
    let msg = inst.failure().unwrap();
    assert!(msg.contains("se cerró inesperadamente"), "{msg}");
    assert!(wait_until(Duration::from_secs(1), || FAILED.lock().unwrap().iter().any(|(id, _)| id == "slot-crash")));
    assert_eq!(run_chain(&mut chain), 0.5);
    assert!(inst.params().is_empty());
    assert!(inst.open_editor().is_err());

    // Reconstruir la cadena vuelve a cargarlo con el último estado guardado.
    let mut chain = Chain::new(&slots, 48_000);
    let fresh = instance("slot-crash").unwrap();
    assert!(!Arc::ptr_eq(&fresh, &inst));
    assert!(fresh.failure().is_none());
    assert!((run_chain(&mut chain) - MINUS_6_DB).abs() < 1e-5);
    retain_instances(|id| id != "slot-crash");
}

#[test]
fn a_hung_audio_thread_is_skipped_and_then_terminated() {
    let f = fixture();
    let r = plugin(&f.clap, "org.musicnova.test-crash");
    let inst = Instance::load_with(&r, 48_000, true).unwrap();
    let mut p = PluginProcessor::new(inst.clone());
    inst.set_param(0, -60.0); // el audio del plugin se cuelga
    let t = Instant::now();
    assert_eq!(run(&mut p), 0.5);
    let first = t.elapsed();
    assert!(first < Duration::from_millis(600), "un bloque espera como mucho el límite de tiempo real ({first:?})");
    // Los bloques siguientes no esperan nada mientras siga colgado.
    let t = Instant::now();
    assert_eq!(run(&mut p), 0.5);
    assert!(t.elapsed() < Duration::from_millis(20));
    // Si no se recupera, se termina su proceso.
    assert!(wait_until(Duration::from_secs(10), || {
        run(&mut p);
        inst.failure().is_some()
    }));
    assert!(inst.failure().unwrap().contains("dejó de responder"));
}

#[test]
fn a_hung_main_thread_does_not_freeze_the_caller() {
    let f = fixture();
    let r = plugin(&f.clap, "org.musicnova.test-crash");
    let inst = Instance::load_with(&r, 48_000, true).unwrap();
    let mut p = PluginProcessor::new(inst.clone());
    inst.set_param(1, 1.0); // con «Invertir», guardar el estado cuelga su hilo principal
    run(&mut p);
    let t = Instant::now();
    assert!(inst.state_base64().is_none());
    let took = t.elapsed();
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert!(wait_until(Duration::from_secs(2), || inst.failure().is_some()));
    assert!(inst.failure().unwrap().contains("dejó de responder"));
    assert_eq!(run(&mut p), 0.5, "después, el audio pasa sin procesar");
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Ventana del plugin y proceso que la creó.
fn editor_window(title: &str) -> Option<(isize, u32)> {
    let (class, title) = (wide("MusicNovaPluginWindow"), wide(title));
    let hwnd = unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) };
    if hwnd.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    Some((hwnd as isize, pid))
}

#[test]
fn the_editor_lives_in_the_plugin_process() {
    let f = fixture();
    let r = plugin(&f.clap, "org.musicnova.test-gain");
    let _chain = Chain::new(&[slot("slot-editor", &r)], 48_000);
    let inst = instance("slot-editor").unwrap();
    let title = format!("{} · MusicNova", r.name);
    inst.open_editor().unwrap();
    assert!(inst.editor_open());
    let (hwnd, pid) = editor_window(&title).expect("ventana del plugin");
    assert_ne!(pid, std::process::id(), "la ventana es del proceso anfitrión");
    inst.close_editor();
    assert!(wait_until(Duration::from_secs(2), || editor_window(&title).is_none()));
    assert!(!inst.editor_open());

    // El usuario la cierra con la X: llega el aviso con el id del efecto.
    inst.open_editor().unwrap();
    let (hwnd2, _) = editor_window(&title).expect("ventana del plugin");
    assert_ne!(hwnd, 0);
    unsafe { PostMessageW(hwnd2 as _, WM_CLOSE, 0, 0) };
    assert!(wait_until(Duration::from_secs(2), || CLOSED.lock().unwrap().iter().any(|s| s == "slot-editor")));
    assert!(wait_until(Duration::from_secs(1), || !inst.editor_open()));
    retain_instances(|id| id != "slot-editor");
}

#[test]
fn closing_the_instance_ends_its_process() {
    let f = fixture();
    let inst = Instance::load(&plugin(&f.clap, "org.musicnova.test-delay"), 48_000).unwrap();
    inst.open_editor().unwrap();
    let (_, pid) = editor_window("MusicNova Test Delay · MusicNova").expect("ventana");
    drop(inst);
    let gone = wait_until(Duration::from_secs(5), || {
        let out = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        !String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    });
    assert!(gone, "el proceso anfitrión {pid} sigue vivo");
}

/// Automatización de un parámetro de plugin: la ganancia del plugin de prueba
/// sigue la curva en la cadena en vivo y en la mezcla offline (exportación).
fn check_plugin_automation(r: &PluginRef, slot_id: &str) {
    use mn_core::automation::{AutoPoint, AutomationLane, AutomationTarget, ChannelAutomation};
    use mn_core::mix::{mixdown, ClipFrames, MixInput};
    use mn_core::project::MasterSettings;
    let rate = 48_000;
    let s = EffectSlot { sidechain: None, id: slot_id.into(), enabled: true, params: EffectParams::Plugin, plugin: Some(r.clone()) };
    let target = AutomationTarget::Plugin { slot: slot_id.into(), param: 0, name: "Ganancia".into(), min: -60.0, max: 12.0 };
    // -6 dB durante el primer medio segundo, 0 dB después.
    let points = vec![AutoPoint { time: 0.0, value: -6.0 }, AutoPoint { time: 0.5, value: -6.0 }, AutoPoint { time: 0.5, value: 0.0 }];
    let lanes = vec![AutomationLane { id: uuid::Uuid::new_v4(), target: target.clone(), enabled: true, points }];
    let effects = std::slice::from_ref(&s);
    assert_eq!(mn_core::automation::sanitize_lanes(lanes.clone(), effects).len(), 1, "el destino es válido");
    assert_eq!(target.clamp(effects, 40.0), Some(12.0));
    let auto = ChannelAutomation::build(&lanes, effects, rate).expect("automatización");

    // En vivo: se aplica al inicio de cada bloque.
    let mut chain = Chain::new(effects, rate);
    let mut at = |frame: u64| {
        auto.apply_effects(&mut chain, frame);
        let mut buf = vec![0.5f32; 2 * 256];
        chain.process(&mut buf);
        buf[0]
    };
    assert!((at(0) - 0.5 * 0.501_187).abs() < 1e-4, "-6 dB al principio");
    assert!((at(30_000) - 0.5).abs() < 1e-4, "0 dB después de 0,5 s");
    assert!((at(256) - 0.5 * 0.501_187).abs() < 1e-4, "al volver atrás, otra vez -6 dB");

    // Exportación: un segundo de 0,5 constante.
    let data = vec![0.5f32; rate as usize * 2];
    let clips = [ClipFrames::whole(0, rate as u64)];
    let input = MixInput { data: &data, clips: &clips, gain_db: 0.0, pan: 0.0, mute: false, solo: false, effects, automation: Some(&auto), sidechain: None };
    let out = mixdown(&[input], &MasterSettings::default(), None, rate).data;
    let (early, late) = (out[2 * 1000], out[2 * 40_000]);
    assert!(late > early * 1.8, "la exportación sigue la curva: {early} → {late}");
}

#[test]
fn plugin_automation_reaches_the_other_process() {
    let f = fixture();
    check_plugin_automation(&plugin(&f.clap, "org.musicnova.test-gain"), "slot-auto-remote");
    retain_instances(|id| id != "slot-auto-remote");
}

/// Plugin instrumento tocado en vivo: notas por bloque (como el motor) → La4.
fn check_live_plugin_instrument(r: &PluginRef, key: &str) {
    use mn_fx::LiveNote;
    let rate = 48_000;
    let mut inst = mn_plugin::live_instrument(r, rate, key).expect("instrumento en vivo");
    assert!(instance(key).is_some(), "registrado para abrir su interfaz");
    let mut buf = vec![0.0f32; 2 * 512];
    inst.render(&[], &mut buf);
    assert!(buf.iter().all(|&s| s == 0.0));
    inst.render(&[LiveNote { time: 100, key: 69, velocity: 127, on: true }], &mut buf);
    assert!(buf[..200].iter().all(|&s| s == 0.0), "empieza en su frame del bloque");
    let mut out = Vec::new();
    for _ in 0..20 {
        inst.render(&[], &mut buf);
        out.extend(buf.iter().step_by(2).copied());
    }
    let hz = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / (out.len() as f64 / rate as f64);
    assert!((hz - 440.0).abs() < 10.0, "{hz}");
    // Rueda de tono arriba del todo: el sintetizador de prueba sube dos semitonos.
    inst.control(mn_fx::LiveControl::PitchBend(1.0));
    out.clear();
    for _ in 0..20 {
        inst.render(&[], &mut buf);
        out.extend(buf.iter().step_by(2).copied());
    }
    let hz = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / (out.len() as f64 / rate as f64);
    assert!((hz - 493.9).abs() < 10.0, "con la rueda de tono: {hz}");
    inst.control(mn_fx::LiveControl::PitchBend(0.0));
    inst.all_notes_off();
    for _ in 0..4 {
        inst.render(&[], &mut buf);
    }
    assert!(buf.iter().all(|&s| s.abs() < 1e-6), "suelta las notas pulsadas");
    // `retain_instances` no suelta los instrumentos en vivo; `release_instrument`, sí.
    retain_instances(|_| false);
    assert!(instance(key).is_some());
    mn_plugin::release_instrument(key);
    assert!(instance(key).is_none());
}

#[test]
fn instrument_plugins_play_live_from_another_process() {
    let f = fixture();
    check_live_plugin_instrument(&plugin(&f.clap, "org.musicnova.test-synth"), "inst:live-remote");
}
