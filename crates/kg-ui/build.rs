//! 把 assets/keyguard.ico 编进 exe 的图标资源。
//!
//! 只用 Windows SDK 自带的 rc.exe，找不到就静默跳过——图标是装饰，不能因为它让构建失败。

use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    if let Ok(rd) = std::fs::read_dir(kits) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let ver: Vec<u32> = e
                .file_name()
                .to_string_lossy()
                .split('.')
                .filter_map(|s| s.parse().ok())
                .collect();
            let rc = p.join("x64").join("rc.exe");
            if rc.is_file() && best.as_ref().map_or(true, |(v, _)| ver > *v) {
                best = Some((ver, rc));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn main() {
    println!("cargo:rerun-if-changed=../../assets/app.rc");
    println!("cargo:rerun-if-changed=../../assets/keyguard.ico");

    let Some(rc) = find_rc() else { return };
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let res = out.join("app.res");

    let status = Command::new(&rc)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg("app.rc")
        .status();

    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg={}", res.display()),
        _ => println!("cargo:warning=rc.exe 没能生成图标资源，exe 将使用系统默认图标"),
    }
}
