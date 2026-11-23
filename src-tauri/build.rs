use std::path::PathBuf;
use std::process::Command;

fn main() {
    #[cfg(target_os = "macos")]
    build_gestures();
    tauri_build::build();
}

#[cfg(target_os = "macos")]
fn build_gestures() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let package = manifest.join("../native/gestures");
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let configuration = if profile == "release" { "release" } else { "debug" };
    // swift build follows the runner, not the Rust --target. An Intel release
    // built on an Apple Silicon runner would otherwise ship an arm64 library
    // and the trackpad would never load.
    let arch = swift_arch();
    let triple = format!("{arch}-apple-macosx13.0");
    let status = Command::new("swift")
        .args(["build", "-c", configuration, "--triple", &triple, "--package-path"])
        .arg(&package)
        .status()
        .expect("swift is required to build Tap on macOS");
    if !status.success() {
        panic!("Swift trackpad library failed to build for {triple}");
    }
    let dylib = package
        .join(".build")
        .join(configuration)
        .join("libTapGestures.dylib");
    require_dylib_arch(&dylib, arch);
    // tauri.conf.json lists this path so the Mac bundle can copy the library
    // into the app. Debug builds still have to leave a file there.
    let bundled = package
        .join(".build/release/libTapGestures.dylib");
    if dylib.exists() && dylib != bundled && !bundled.exists() {
        if let Some(parent) = bundled.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(&dylib, &bundled);
    }
    println!("cargo:rustc-env=TAP_GESTURES_LIB={}", dylib.display());
    println!("cargo:rerun-if-changed=../native/gestures/Package.swift");
    println!("cargo:rerun-if-changed=../native/gestures/Sources/TapGestures");

    let target = std::env::var("OUT_DIR")
        .ok()
        .and_then(|_| std::env::var("CARGO_TARGET_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| manifest.join("target"));
    let dest_dir = target.join(&profile);
    let _ = std::fs::create_dir_all(&dest_dir);
    let dest = dest_dir.join("libTapGestures.dylib");
    if dylib.exists() {
        let _ = std::fs::copy(&dylib, &dest);
    }
}

#[cfg(target_os = "macos")]
fn swift_arch() -> &'static str {
    match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86_64",
        Ok("aarch64") => "arm64",
        other => panic!("Tap's trackpad library cannot be built for {other:?}"),
    }
}

#[cfg(target_os = "macos")]
fn require_dylib_arch(dylib: &std::path::Path, arch: &str) {
    if !dylib.is_file() {
        panic!("trackpad library was not produced at {}", dylib.display());
    }
    let output = Command::new("file")
        .arg(dylib)
        .output()
        .expect("file is required to check the trackpad library architecture");
    let text = String::from_utf8_lossy(&output.stdout);
    let matches = match arch {
        "x86_64" => text.contains("x86_64"),
        "arm64" => text.contains("arm64"),
        _ => false,
    };
    if !output.status.success() || !matches {
        panic!(
            "trackpad library at {} is {text}, expected {arch}",
            dylib.display()
        );
    }
}
