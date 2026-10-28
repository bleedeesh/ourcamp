//! Generates Tauri's build-time context (config, capabilities, icons).

fn main() {
    // The icons end up in the executable twice: as its Windows resource (taskbar,
    // Explorer) and inside `generate_context!` (window and tray icon). tauri-build does
    // not list them as inputs, so a new icon set would leave a stale build script output
    // behind — and with it the old icons. Listing the folder makes cargo rerun this
    // script, and rebuild the crate, whenever an icon changes (ADR-0027).
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build();
}
