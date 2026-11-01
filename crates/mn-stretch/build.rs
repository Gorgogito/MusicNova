// Compila Signalsmith Stretch (solo cabeceras C++) con una envoltura C.
// No usa bindgen para no depender de libclang: las declaraciones FFI están en src/lib.rs.
fn main() {
    println!("cargo::rerun-if-changed=vendor");
    cc::Build::new()
        .file("vendor/wrapper/wrapper.cpp")
        .include("vendor/signalsmith-stretch")
        .include("vendor")
        .cpp(true)
        .std("c++14")
        .opt_level(3)
        .warnings(false)
        .compile("signalsmith-stretch");
}
