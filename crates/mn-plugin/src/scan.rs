//! Búsqueda de plugins en disco.

use std::path::{Path, PathBuf};

use crate::PluginDescriptor;

/// Carpetas estándar de plugins CLAP y VST3 en Windows (existan o no).
pub fn default_folders() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(common) = env("COMMONPROGRAMFILES") {
        out.push(common.join("CLAP"));
        out.push(common.join("VST3"));
    }
    if let Some(local) = env("LOCALAPPDATA") {
        out.push(local.join("Programs").join("Common").join("CLAP"));
        out.push(local.join("Programs").join("Common").join("VST3"));
    }
    if let Some(extra) = std::env::var_os("CLAP_PATH") {
        out.extend(std::env::split_paths(&extra));
    }
    out
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

/// Archivos `.clap` y archivos o paquetes `.vst3` dentro de `folders` (recursivo).
pub fn find_candidates(folders: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if has_ext(&p, "clap") && p.is_file() {
                out.push(p);
            } else if has_ext(&p, "vst3") {
                // Un paquete .vst3 es una carpeta: se trata como un único plugin.
                out.push(p);
            } else if p.is_dir() && depth < 6 {
                walk(&p, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for f in folders {
        if has_ext(f, "clap") || has_ext(f, "vst3") {
            out.push(f.clone());
        } else {
            walk(f, 0, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Lista los efectos de un archivo. Carga código del plugin: la aplicación lo
/// llama en un proceso aparte para que un plugin defectuoso no la tumbe.
pub fn scan_file(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    if has_ext(path, "clap") {
        crate::clap::scan(path)
    } else if has_ext(path, "vst3") {
        crate::vst3::scan(path)
    } else {
        Err("no es un archivo de plugin (.clap o .vst3)".into())
    }
}

/// DLL dentro de un paquete VST3 (`X.vst3/Contents/x86_64-win/X.vst3`) o el
/// propio archivo si es un VST3 de un solo archivo.
pub(crate) fn vst3_binary(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let arch = if cfg!(target_arch = "aarch64") { "arm64-win" } else { "x86_64-win" };
    let dir = path.join("Contents").join(arch);
    let name = path.file_name().ok_or("ruta de plugin no válida")?;
    let candidate = dir.join(name);
    if candidate.is_file() {
        return Ok(candidate);
    }
    // Algunos paquetes nombran la DLL distinto de la carpeta.
    std::fs::read_dir(&dir)
        .ok()
        .and_then(|mut it| it.find_map(|e| e.ok().map(|e| e.path()).filter(|p| has_ext(p, "vst3") && p.is_file())))
        .ok_or_else(|| format!("el paquete {} no contiene una DLL para {arch}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_clap_files_and_vst3_bundles() {
        let root = std::env::temp_dir().join(format!("mn_scan_{}", std::process::id()));
        let bundle = root.join("Vendor").join("Cosa.vst3").join("Contents").join("x86_64-win");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("Cosa.vst3"), b"x").unwrap();
        std::fs::write(root.join("Uno.clap"), b"x").unwrap();
        std::fs::write(root.join("leeme.txt"), b"x").unwrap();
        let found = find_candidates(&[root.clone(), root.join("no-existe")]);
        assert_eq!(found, vec![root.join("Uno.clap"), root.join("Vendor").join("Cosa.vst3")]);
        assert_eq!(vst3_binary(&found[1]).unwrap(), bundle.join("Cosa.vst3"));
        std::fs::remove_dir_all(&root).ok();
    }
}
