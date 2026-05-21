fn main() {
    // Link Windows Restart Manager library for DuckDB on Windows
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-lib=rstrtmgr");
    }
    tauri_build::build();
}
