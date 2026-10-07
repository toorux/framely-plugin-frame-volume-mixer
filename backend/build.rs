use std::{env, path::PathBuf, process::Command};
fn main() {
    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();
    let prefix = if target != host && target.starts_with("aarch64-") {
        "aarch64-linux-gnu-"
    } else {
        ""
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let compiler = env::var("CXX").unwrap_or_else(|_| format!("{prefix}g++"));
    let archiver = env::var("AR").unwrap_or_else(|_| format!("{prefix}ar"));
    assert!(Command::new(compiler)
        .args(["-std=c++17", "-O2", "-fPIC", "-c", "src/vr_focus.cpp", "-o"])
        .arg(out.join("vr_focus.o"))
        .status()
        .unwrap()
        .success());
    assert!(Command::new(archiver)
        .arg("crs")
        .arg(out.join("libvr_focus.a"))
        .arg(out.join("vr_focus.o"))
        .status()
        .unwrap()
        .success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=vr_focus");
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rerun-if-changed=src/vr_focus.cpp");
    println!("cargo:rerun-if-changed=vendor/openvr/openvr.h");
    println!("cargo:rerun-if-env-changed=CXX");
    println!("cargo:rerun-if-env-changed=AR");
}
