fn main() {
    // Ensure Windows .rc icon resources are always re-embedded when icons change
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
