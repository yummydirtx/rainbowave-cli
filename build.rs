use std::{env, path::PathBuf, process::Command};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos_audio_bridge();

        let manifest = PathBuf::from(
            env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory is available"),
        );
        let plist = manifest.join("assets/Info.plist");
        println!("cargo:rerun-if-changed={}", plist.display());
        println!(
            "cargo:rustc-link-arg-bin=rainbowave=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );

        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=Foundation");
        // The picker is only called after a macOS 14 runtime check. Weak linking keeps
        // the ordinary animation launchable on older macOS versions.
        println!("cargo:rustc-link-arg-bin=rainbowave=-Wl,-weak_framework,ScreenCaptureKit");
    }
}

fn build_macos_audio_bridge() {
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory is available"));
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("output directory is available"));
    let source = manifest.join("src/macos_audio.m");
    let object = output.join("macos_audio.o");
    let library = output.join("librainbowave_macos_audio.a");
    let target = env::var("TARGET").expect("target triple is available");
    let architecture = if target.starts_with("aarch64-") {
        "arm64"
    } else if target.starts_with("x86_64-") {
        "x86_64"
    } else {
        panic!("unsupported macOS architecture in target {target}");
    };

    println!("cargo:rerun-if-changed={}", source.display());
    run(
        Command::new("xcrun")
            .args(["--sdk", "macosx", "clang", "-c", "-fobjc-arc", "-fblocks"])
            .args(["-arch", architecture])
            .arg("-mmacosx-version-min=11.0")
            .arg("-Wno-unguarded-availability-new")
            .arg(&source)
            .arg("-o")
            .arg(&object),
        "compile the ScreenCaptureKit bridge",
    );
    run(
        Command::new("xcrun")
            .args(["--sdk", "macosx", "libtool", "-static", "-o"])
            .arg(&library)
            .arg(&object),
        "archive the ScreenCaptureKit bridge",
    );

    println!("cargo:rustc-link-search=native={}", output.display());
    println!("cargo:rustc-link-lib=static=rainbowave_macos_audio");
}

fn run(command: &mut Command, description: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("could not {description}: {error}"));
    assert!(status.success(), "failed to {description}: {status}");
}
