//! On iOS: compiles the SwiftUI deck (`swift/`) into a static library the app links.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=swift");
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    let (sdk, triple, platform) = match target.as_str() {
        "aarch64-apple-ios-sim" => (
            "iphonesimulator",
            "arm64-apple-ios17.0-simulator",
            "iphonesimulator",
        ),
        "aarch64-apple-ios" => ("iphoneos", "arm64-apple-ios17.0", "iphoneos"),
        _ => return,
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let run = |args: &[&str]| {
        let output = Command::new("xcrun").args(args).output().expect("xcrun");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let sdk_path = run(&["--sdk", sdk, "--show-sdk-path"]);
    let mut sources: Vec<PathBuf> = std::fs::read_dir("swift")
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "swift"))
        .collect();
    sources.sort();
    let lib = out.join("libHorizonDeck.a");
    let mut args = vec![
        "--sdk".to_owned(),
        sdk.to_owned(),
        "swiftc".to_owned(),
        "-parse-as-library".to_owned(),
        "-emit-library".to_owned(),
        "-static".to_owned(),
        "-O".to_owned(),
        "-module-name".to_owned(),
        "HorizonDeck".to_owned(),
        "-target".to_owned(),
        triple.to_owned(),
        "-sdk".to_owned(),
        sdk_path.clone(),
        "-o".to_owned(),
        lib.display().to_string(),
    ];
    args.extend(sources.iter().map(|p| p.display().to_string()));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&args);
    // The Swift runtime: in the OS (the SDK's stubs) and the toolchain's back-deployment shims.
    let swiftc = run(&["--sdk", sdk, "--find", "swiftc"]);
    let toolchain = PathBuf::from(swiftc)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-search=native={sdk_path}/usr/lib/swift");
    println!(
        "cargo:rustc-link-search=native={}",
        toolchain.join("lib/swift").join(platform).display()
    );
    println!("cargo:rustc-link-lib=static=HorizonDeck");
    for framework in ["SwiftUI", "UIKit", "Foundation"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
