//! Comprueba el aislamiento con un plugin real: lo carga en su propio proceso
//! (anfitrión `mn-plugin-host`, junto a este ejemplo en `target/<perfil>`),
//! procesa audio, abre su interfaz y mide que quien llama nunca se queda
//! colgado aunque el plugin bloquee su hilo principal.
//!
//! `cargo build -p mn-plugin --bins --examples && target\debug\examples\isolation <plugin> [id]`

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mn_fx::Processor;
use mn_plugin::{scan_file, Instance, PluginProcessor};

fn level(p: &mut PluginProcessor) -> (f32, Duration) {
    // Un clic al principio; la reverberación deja cola en el resto del bloque.
    let mut buf = vec![0.0f32; 2 * 1024];
    buf[0] = 1.0;
    buf[1] = 1.0;
    let t = Instant::now();
    p.process(&mut buf);
    let e = t.elapsed();
    (buf[2..].iter().map(|s| s.abs()).fold(0.0, f32::max), e)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = PathBuf::from(args.get(1).expect("uso: isolation <plugin> [id]"));
    let exe = std::env::current_exe().unwrap();
    let host = exe.parent().unwrap().parent().unwrap().join("mn-plugin-host.exe");
    assert!(Path::new(&host).exists(), "falta {}", host.display());
    mn_plugin::set_host_command(host, vec![mn_plugin::HOST_FLAG.into()]);

    let found = scan_file(&path).expect("escaneo");
    let d = match args.get(2) {
        Some(id) => found.iter().find(|d| &d.id == id).expect("id"),
        None => &found[0],
    };
    println!("plugin: {} ({:?})", d.name, d.format);
    let t = Instant::now();
    let inst = Instance::load_with(&d.to_ref(), 48_000, true).expect("carga");
    println!("cargado en otro proceso en {:?}; {} parámetros", t.elapsed(), inst.params().len());
    let mut p = PluginProcessor::new(inst.clone());
    for _ in 0..20 {
        level(&mut p);
    }
    let (wet, e) = level(&mut p);
    println!("cola tras un clic: {wet:.4} (bloque en {e:?})");

    let t = Instant::now();
    println!("abrir interfaz: {:?} en {:?}", inst.open_editor(), t.elapsed());
    for i in 0..12 {
        std::thread::sleep(Duration::from_millis(500));
        let t = Instant::now();
        let n = inst.params().len();
        let (wet, e) = level(&mut p);
        println!(
            "t={:>4} ms: params() → {n} en {:?}; audio: cola {wet:.4} en {e:?}; fallo: {:?}",
            (i + 1) * 500,
            t.elapsed(),
            inst.failure()
        );
        if inst.failure().is_some() {
            break;
        }
    }
    inst.close_editor();
}
