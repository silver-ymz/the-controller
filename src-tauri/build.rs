use std::path::Path;

fn main() {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    println!("cargo:rustc-env=BUILD_DATE={}", today);

    // TAURI_ENV_TARGET_TRIPLE is only set by the Tauri CLI (tauri dev / tauri build).
    // When absent, we're running under `cargo test`, `cargo clippy`, etc. —
    // create empty sidecar placeholders so tauri_build validation passes.
    // When present, skip placeholders so a missing real binary fails loudly
    // instead of silently producing a broken app.
    if std::env::var("TAURI_ENV_TARGET_TRIPLE").is_err() {
        let binaries_dir = Path::new("binaries");
        let target = std::env::var("TARGET").unwrap_or_default();
        if !target.is_empty() {
            let _ = std::fs::create_dir_all(binaries_dir);
            for name in ["controller-cli", "pty-broker"] {
                let path = binaries_dir.join(format!("{name}-{target}"));
                if !path.exists() {
                    let _ = std::fs::File::create(&path);
                }
            }
        }
    }

    tauri_build::build()
}
