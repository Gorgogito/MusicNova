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
    assert_eq!(clap.len(), 1);
    assert_eq!(clap[0].id, "org.musicnova.test-gain");
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

    inst.set_param(0, -6.0);
    let v = run(&mut p);
    assert!((v - 0.5 * 0.501_187).abs() < 1e-4, "{v}");
    assert_eq!(inst.params()[0].text, "-6.0 dB");
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
    let slot = EffectSlot { id: "slot-chain".into(), enabled: true, params: EffectParams::Plugin, plugin: Some(r.clone()) };

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
    let slot = EffectSlot {
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
