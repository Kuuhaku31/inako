
use std::env;
use std::path::PathBuf;
use std::process::Command;


/// 编译 Blueprint 布局并配置 Cargo 资源变更跟踪.
fn main() {

    println!("cargo:rerun-if-changed=src/gui/window.blp");
    println!("cargo:rerun-if-changed=src/gui/style.css");

    let output = PathBuf::from(env::var_os("OUT_DIR")
        .expect("OUT_DIR is not set"))
        .join("window.ui");
    let status = Command::new("blueprint-compiler")
        .args(["compile", "--output"])
        .arg(&output)
        .arg("src/gui/window.blp")
        .status()
        .expect("blueprint-compiler is required to build the GUI");
    assert!(status.success(), "failed to compile src/gui/window.blp");
}
