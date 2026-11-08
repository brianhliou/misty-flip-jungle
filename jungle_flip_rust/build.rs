fn main() {
    #[cfg(feature = "pyext")]
    {
        // Cargo needs the same platform extension-linker flags as maturin.
        pyo3_build_config::add_extension_module_link_args();

        // Release LTO can retain Python exports from the rlib in integration-test
        // executables. Link only those tests against the configured interpreter;
        // the extension itself still resolves Python symbols from its host.
        let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        if target == "macos" || target == "linux" {
            let config = pyo3_build_config::get();
            if let Some(dir) = config.lib_dir() {
                println!("cargo:rustc-link-arg-tests=-L{dir}");
                println!("cargo:rustc-link-arg-tests=-Wl,-rpath,{dir}");
            }
            if let Some(name) = config.lib_name() {
                println!("cargo:rustc-link-arg-tests=-l{name}");
            }
        }
    }
}
