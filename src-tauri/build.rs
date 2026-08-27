#[cfg(windows)]
fn build_taskbar_tap() {
    use std::{env, path::PathBuf, process::Command};

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source = manifest_dir.join("taskbar-helper/tap.cpp");
    let exports = manifest_dir.join("taskbar-helper/tap.def");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out_dir.join("tbwidget_taskbar_tap.obj");
    let dll = out_dir.join("tbwidget_taskbar_tap.dll");

    let compiler = cc::Build::new().cpp(true).get_compiler();
    assert!(compiler.is_like_msvc(), "taskbar TAP requires MSVC");

    let compile_status = compiler
        .to_command()
        .args(["/nologo", "/std:c++20", "/EHsc", "/permissive-", "/c"])
        .arg(&source)
        .arg(format!("/Fo{}", object.display()))
        .status()
        .expect("failed to launch MSVC for taskbar TAP");
    assert!(compile_status.success(), "failed to compile taskbar TAP");

    let linker = compiler.path().with_file_name("link.exe");
    let mut link = Command::new(linker);
    for (key, value) in compiler.env() {
        link.env(key, value);
    }
    let link_status = link
        .args(["/nologo", "/DLL", "/INCREMENTAL:NO"])
        .arg(format!("/OUT:{}", dll.display()))
        .arg(format!("/DEF:{}", exports.display()))
        .arg(&object)
        .args([
            "windowsapp.lib",
            "runtimeobject.lib",
            "ole32.lib",
            "user32.lib",
        ])
        .status()
        .expect("failed to launch linker for taskbar TAP");
    assert!(link_status.success(), "failed to link taskbar TAP");

    println!("cargo:rustc-env=TBWIDGET_TAP_DLL={}", dll.display());
}

fn main() {
    println!("cargo:rerun-if-changed=taskbar-helper/tap.cpp");
    println!("cargo:rerun-if-changed=taskbar-helper/tap.def");

    #[cfg(windows)]
    build_taskbar_tap();

    tauri_build::build()
}
