//! Generates Tauri's build-time context (config, capabilities, icons).

fn main() {
    // The icons end up in the executable twice: as its Windows resource (taskbar,
    // Explorer) and inside `generate_context!` (window and tray icon). tauri-build does
    // not list them as inputs, so a new icon set would leave a stale build script output
    // behind — and with it the old icons. Listing the folder makes cargo rerun this
    // script, and rebuild the crate, whenever an icon changes (ADR-0027).
    println!("cargo:rerun-if-changed=icons");
    // Media Foundation is linked lazily: Windows without it ("N" editions, servers) must
    // still start the app; only streams need it, and they say so (ADR-0049).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfplat.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
    tauri_build::build();
}
