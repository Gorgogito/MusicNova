//! Pruebas del alojamiento con un plugin real: `mn-testplugin` compilado como
//! DLL y copiado como `.clap`, como `.vst3` de un archivo y como paquete `.vst3`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use mn_fx::{Chain, EffectParams, EffectSlot, PluginRef, Processor};
use mn_plugin::{find_candidates, instance, load_error, retain_instances, scan_file, sync_states, Instance, PluginFormat, PluginProcessor};

struct Fixture {
    clap: PathBuf,
    vst3: PathBuf,
    bundle: PathBuf,
}

/// Compila el plugin de prueba una vez y lo copia con cada extensión.
fn fixture() -> &'static Fixture {
    static F: OnceLock<Fixture> = OnceLock::new();
    F.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
        let status = Command::new(env!("CARGO"))
            .args(["build", "-p", "mn-testplugin"])
            .current_dir(root)
            .status()
            .expect("cargo build");
        assert!(status.success(), "no se pudo compilar el plugin de prueba");
        let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
        let dll = target.join("debug").join("mn_testplugin.dll");
        let dir = std::env::temp_dir().join(format!("mn_plugins_{}", std::process::id()));
        let bundle_bin = dir.join("Bundle.vst3").join("Contents").join("x86_64-win");
        std::fs::create_dir_all(&bundle_bin).unwrap();
        let f = Fixture { clap: dir.join("MusicNovaTest.clap"), vst3: dir.join("MusicNovaTest.vst3"), bundle: dir.join("Bundle.vst3") };
        std::fs::copy(&dll, &f.clap).unwrap();
        std::fs::copy(&dll, &f.vst3).unwrap();
        std::fs::copy(&dll, bundle_bin.join("Bundle.vst3")).unwrap();
        mn_plugin::install();
        f
    })
}

fn plugin_ref(path: &Path, format: PluginFormat) -> PluginRef {
    let d = scan_file(path).unwrap().remove(0);
    assert_eq!(d.format, format);
    d.to_ref()
}

/// Procesa un bloque de 0,5 constante y devuelve la primera muestra izquierda.
fn run(p: &mut dyn Processor) -> f32 {
    let mut buf = vec![0.5f32; 2 * 300];
    p.process(&mut buf);
    assert!(buf.chunks_exact(2).all(|f| f[0] == f[1]), "estéreo coherente");
    buf[0]
}

#[test]
fn scans_clap_vst3_file_and_bundle() {
    let f = fixture();
    let found = find_candidates(&[f.clap.parent().unwrap().to_path_buf()]);
    assert_eq!(found.len(), 3, "{found:?}");

    let clap = scan_file(&f.clap).unwrap();
    assert_eq!(clap.len(), 4);
    assert!(clap.iter().any(|p| p.id == "org.musicnova.test-gain"));
    assert!(clap.iter().any(|p| p.id == "org.musicnova.test-delay"));
    assert_eq!(clap[0].vendor, "MusicNova");
    assert!(clap[0].category.contains("audio-effect"));

    for path in [&f.vst3, &f.bundle] {
        let v = scan_file(path).unwrap();
        assert_eq!(v.len(), 1, "solo la clase de audio, no la del controlador");
        assert_eq!(v[0].name, "MusicNova Test Gain (VST3)");
        assert_eq!(v[0].vendor, "MusicNova");
        assert_eq!(v[0].id.len(), 32);
    }
    assert!(scan_file(Path::new("C:/no/existe.clap")).is_err());
}

#[test]
fn clap_processes_params_and_state() {
    let f = fixture();
    let r = plugin_ref(&f.clap, PluginFormat::Clap);
    let inst = Instance::load(&r, 48_000).unwrap();
    let mut p = PluginProcessor::new(inst.clone());
    assert_eq!(run(&mut p), 0.5, "0 dB por defecto");

    let params = inst.params();
    assert_eq!(params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Ganancia", "Invertir"]);
    assert!(params[1].stepped);
    assert_eq!((params[0].steps, params[1].steps), (0, 1));

    inst.set_param(0, -6.0);
    let v = run(&mut p);
    assert!((v - 0.5 * 0.501_187).abs() < 1e-4, "{v}");
    assert_eq!(inst.params()[0].text, "-6.0 dB");
    assert_eq!(inst.param_from_text(0, "-3,5 dB"), Some(-3.5), "texto escrito por el usuario");
    assert_eq!(inst.param_from_text(1, "Sí"), Some(1.0));
    assert_eq!(inst.param_from_text(0, "mucho"), None);
    inst.set_param(1, 1.0);
    assert!(run(&mut p) < 0.0, "fase invertida");

    // El estado guardado reproduce el mismo sonido en otra instancia.
    let mut saved = r.clone();
    saved.state = inst.state_base64();
    assert!(saved.state.is_some());
    let other = Instance::load(&saved, 44_100).unwrap();
    let w = run(&mut PluginProcessor::new(other));
    assert!((w + 0.5 * 0.501_187).abs() < 1e-4, "{w}");
}

#[test]
fn vst3_processes_params_and_state() {
    let f = fixture();
    for path in [&f.vst3, &f.bundle] {
        let r = plugin_ref(path, PluginFormat::Vst3);
        let inst = Instance::load(&r, 48_000).unwrap();
        let mut p = PluginProcessor::new(inst.clone());
        assert_eq!(run(&mut p), 0.5, "ganancia 1 por defecto");
        let params = inst.params();
        assert_eq!(params.len(), 1);
        assert_eq!((params[0].min, params[0].max), (0.0, 1.0));

        inst.set_param(params[0].id, 0.25);
        assert_eq!(run(&mut p), 0.125);
        assert_eq!(inst.params()[0].value, 0.25, "el controlador refleja el cambio");
        assert_eq!(inst.param_from_text(params[0].id, "0.75"), Some(0.75));
        assert_eq!(inst.param_from_text(params[0].id, "x"), None);

        // Estado: componente + controlador.
        let mut saved = r.clone();
        saved.state = inst.state_base64();
        let other = Instance::load(&saved, 48_000).unwrap();
        assert_eq!(run(&mut PluginProcessor::new(other.clone())), 0.125);
        assert_eq!(other.params()[0].value, 0.25, "el controlador recibió el estado del componente");
        assert!(other.open_editor().is_err(), "el plugin VST3 de prueba no tiene interfaz");
    }
}

#[test]
fn chains_reuse_live_instances_and_sync_state() {
    let f = fixture();
    let mut r = plugin_ref(&f.clap, PluginFormat::Clap);
    // Estado inicial: -12 dB (se obtiene de una instancia temporal).
    let tmp = Instance::load(&r, 48_000).unwrap();
    tmp.set_param(0, -12.0);
    run(&mut PluginProcessor::new(tmp.clone()));
    r.state = tmp.state_base64();
    let slot = EffectSlot { sidechain: None, id: "slot-chain".into(), enabled: true, params: EffectParams::Plugin, plugin: Some(r.clone()) };

    // Cadena del motor: crea y registra la instancia en vivo.
    let mut live = Chain::new(std::slice::from_ref(&slot), 48_000);
    let mut buf = vec![0.5f32; 600];
    live.process(&mut buf);
    assert!((buf[0] - 0.5 * 0.251_189).abs() < 1e-4, "{}", buf[0]);
    let inst = instance("slot-chain").expect("instancia en vivo");

    // Reconstruir la cadena reutiliza la misma instancia (no se pierde nada).
    let _again = Chain::new(std::slice::from_ref(&slot), 48_000);
    assert!(std::sync::Arc::ptr_eq(&inst, &instance("slot-chain").unwrap()));

    // Cambios en vivo → se copian al proyecto → la exportación los usa.
    inst.set_param(0, 0.0);
    live.process(&mut buf);
    let mut slots = vec![slot.clone()];
    assert!(sync_states(&mut slots));
    let mut offline = Chain::new_offline(&slots, 48_000);
    let mut out = vec![0.5f32; 600];
    offline.process(&mut out);
    assert_eq!(out[0], 0.5, "la exportación usa el estado actual (0 dB)");

    retain_instances(|id| id != "slot-chain");
    assert!(instance("slot-chain").is_none());
}

#[test]
fn missing_plugin_is_bypassed_with_error() {
    fixture();
    let slot = EffectSlot { sidechain: None,
        id: "slot-missing".into(),
        enabled: true,
        params: EffectParams::Plugin,
        plugin: Some(PluginRef {
            format: "clap".into(),
            path: "C:/no/existe/Nada.clap".into(),
            id: "x".into(),
            name: "Nada".into(),
            vendor: String::new(),
            state: None,
        }),
    };
    let mut chain = Chain::new(&[slot], 48_000);
    let mut buf = vec![0.3f32; 64];
    chain.process(&mut buf);
    assert!(buf.iter().all(|&s| s == 0.3), "sin el plugin, la señal pasa intacta");
    assert!(load_error("slot-missing").unwrap().contains("no se pudo cargar"));
}

#[cfg(windows)]
#[test]
fn clap_editor_opens_in_window_and_closes() {
    let f = fixture();
    let inst = Instance::load(&plugin_ref(&f.clap, PluginFormat::Clap), 48_000).unwrap();
    inst.open_editor().unwrap();
    assert!(inst.editor_open());
    inst.open_editor().unwrap(); // segunda vez: la trae al frente
    inst.close_editor();
    assert!(!inst.editor_open());
}

/// Incumplimientos de los estándares que detectó el plugin de prueba en esta
/// copia de la DLL (la misma ruta devuelve el módulo ya cargado).
fn violations(path: &Path) -> u32 {
    let binary = if path.is_dir() { path.join("Contents").join("x86_64-win").join(path.file_name().unwrap()) } else { path.to_path_buf() };
    unsafe {
        let lib = libloading::Library::new(binary).unwrap();
        let f: libloading::Symbol<unsafe extern "C" fn() -> u32> = lib.get(b"mn_test_violations\0").unwrap();
        f()
    }
}

#[test]
fn host_follows_threading_and_output_rules() {
    let f = fixture();
    for (path, format, value) in [(&f.clap, PluginFormat::Clap, -6.0), (&f.vst3, PluginFormat::Vst3, 0.5)] {
        let r = plugin_ref(path, format);
        let _keep = Instance::load(&r, 48_000).unwrap(); // mantiene la DLL cargada para leer el contador
        let inst = Instance::load(&r, 48_000).unwrap();
        let mut p = PluginProcessor::new(inst.clone());
        inst.set_param(inst.params()[0].id, value);
        run(&mut p);
        p.reset();
        run(&mut p);
        assert_eq!(inst.params()[0].value, value, "{format:?}: el controlador conserva el valor");
        drop(p);
        drop(inst); // apagado: stop_processing (CLAP) en un hilo de audio
        assert_eq!(violations(path), 0, "{format:?}: el plugin de prueba detectó incumplimientos (ver stderr)");
    }
}

/// Pista de instrumento: el sintetizador de prueba toca las notas al render.
#[test]
fn renders_notes_with_an_instrument_plugin() {
    let f = fixture();
    let found = scan_file(&f.clap).unwrap();
    let synth = found.iter().find(|d| d.id == "org.musicnova.test-synth").expect("instrumento en el escaneo");
    assert!(synth.instrument);
    assert!(found.iter().filter(|d| d.id != "org.musicnova.test-synth").all(|d| !d.instrument));
    let rate = 48_000;
    // La4 de 0,1 a 0,6 s y Mi5 de 0,3 a 0,8 s (se solapan).
    let notes = [(0.1, 0.6, 69u8, 127u8), (0.3, 0.8, 76u8, 100u8)];
    let audio = mn_plugin::render_instrument(&synth.to_ref(), &notes, rate, 1.0, 0.2).unwrap();
    assert_eq!(audio.len(), (1.2 * rate as f64) as usize * 2);
    let left: Vec<f32> = audio.iter().step_by(2).copied().collect();
    let at = |t: f64| (t * rate as f64) as usize;
    assert!(left[..at(0.099)].iter().all(|&s| s == 0.0), "silencio antes de la primera nota");
    // Solo La4: frecuencia por cruces por cero ascendentes.
    let seg = &left[at(0.12)..at(0.28)];
    let hz = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / 0.16;
    assert!((hz - 440.0).abs() < 10.0, "{hz}");
    // Tras la liberación de la última nota, silencio.
    assert!(left[at(0.85)..].iter().all(|&s| s.abs() < 1e-6));
    assert_eq!(mn_test_violations(), 0);
}

fn mn_test_violations() -> u32 {
    let lib = unsafe { libloading::Library::new(&fixture().clap) }.unwrap();
    let f: libloading::Symbol<unsafe extern "C" fn() -> u32> = unsafe { lib.get(b"mn_test_violations") }.unwrap();
    unsafe { f() }
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
fn plugin_parameters_follow_automation() {
    let f = fixture();
    check_plugin_automation(&plugin_ref(&f.clap, PluginFormat::Clap), "slot-auto");
    retain_instances(|id| id != "slot-auto");
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
fn instrument_plugins_play_live() {
    let f = fixture();
    let synth = scan_file(&f.clap).unwrap().into_iter().find(|d| d.id == "org.musicnova.test-synth").unwrap();
    check_live_plugin_instrument(&synth.to_ref(), "inst:live-local");
}

/// La automatización de un VST3 llega al procesador y también a su controlador
/// (lo que muestran la interfaz del plugin y los parámetros genéricos).
#[test]
fn vst3_controller_follows_automation() {
    use mn_core::automation::{AutoPoint, AutomationLane, AutomationTarget, ChannelAutomation};
    let f = fixture();
    let rate = 48_000;
    for (k, path) in [&f.vst3, &f.bundle].into_iter().enumerate() {
        let slot_id = format!("slot-vst3-auto-{k}");
        let r = plugin_ref(path, PluginFormat::Vst3);
        let s = EffectSlot { sidechain: None, id: slot_id.clone(), enabled: true, params: EffectParams::Plugin, plugin: Some(r) };
        let effects = std::slice::from_ref(&s);
        let mut chain = Chain::new(effects, rate);
        let inst = instance(&slot_id).expect("instancia viva");
        let id = inst.params()[0].id;
        assert_eq!(inst.params()[0].value, 1.0, "valor por defecto");
        let target = AutomationTarget::Plugin { slot: slot_id.clone(), param: id, name: "Gain".into(), min: 0.0, max: 1.0 };
        let points = vec![AutoPoint { time: 0.0, value: 0.25 }, AutoPoint { time: 1.0, value: 0.25 }];
        let lanes = vec![AutomationLane { id: uuid::Uuid::new_v4(), target, enabled: true, points }];
        let auto = ChannelAutomation::build(&lanes, effects, rate).expect("automatización");
        auto.apply_effects(&mut chain, 0);
        let mut buf = vec![0.5f32; 2 * 256];
        chain.process(&mut buf);
        assert_eq!(buf[0], 0.125, "el procesador sigue la curva");
        assert_eq!(inst.params()[0].value, 0.25, "el controlador también");
        drop(chain);
        retain_instances(|i| i != slot_id);
    }
}
