//! Camino de la aplicación: las llamadas de «hilo principal» se envían a un
//! hilo que simula el bucle de la ventana, y el plugin se usa desde otros.
//! (Archivo aparte: el registro del hilo principal es global al proceso.)

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use mn_fx::Processor;
use mn_plugin::{is_main_thread, scan_file, set_main_thread_executor, Instance, PluginProcessor};

fn plugin_path() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    assert!(Command::new(env!("CARGO")).args(["build", "-p", "mn-testplugin"]).current_dir(root).status().unwrap().success());
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let dir = std::env::temp_dir().join(format!("mn_plugins_main_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("Main.clap");
    std::fs::copy(target.join("debug").join("mn_testplugin.dll"), &p).unwrap();
    p
}

#[test]
fn main_thread_calls_are_dispatched_and_never_deadlock() {
    let path = plugin_path();
    let r = scan_file(&path).unwrap().remove(0).to_ref();

    // «Hilo principal»: registra el ejecutor y atiende tareas hasta que se cierre el canal.
    let (ready_tx, ready_rx) = mpsc::channel();
    let main = std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let tx = std::sync::Mutex::new(tx);
        set_main_thread_executor(move |f| {
            let _ = tx.lock().unwrap().send(f);
        });
        assert!(is_main_thread());
        ready_tx.send(()).unwrap();
        let mut handled = 0;
        while let Ok(f) = rx.recv_timeout(Duration::from_secs(3)) {
            f();
            handled += 1;
        }
        handled
    });
    ready_rx.recv().unwrap();
    assert!(!is_main_thread(), "el hilo de la prueba no es el principal");

    // Desde un hilo de trabajo (como la carga de proyectos): cargar, parámetros, interfaz.
    let inst = Instance::load(&r, 48_000).unwrap();
    assert_eq!(inst.params().len(), 2);
    inst.set_param(0, -6.0);
    #[cfg(windows)]
    {
        inst.open_editor().unwrap();
        assert!(inst.editor_open());
    }

    // Desde el «hilo de audio».
    let mut p = PluginProcessor::new(inst.clone());
    let audio = std::thread::spawn(move || {
        let mut buf = vec![0.5f32; 512];
        p.process(&mut buf);
        buf[0]
    });
    let v = audio.join().unwrap();
    assert!((v - 0.5 * 0.501_187).abs() < 1e-4, "{v}");
    assert!(inst.state_base64().is_some());

    // Soltar la última referencia (con la interfaz abierta) apaga el plugin en el hilo principal.
    drop(inst);
    let handled = main.join().unwrap();
    assert!(handled >= 6, "tareas atendidas en el hilo principal: {handled}");
}
