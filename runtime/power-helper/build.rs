//! Gives the helper its code-signing identifier, as the Host's build script
//! does: a bare executable is identified by an embedded `Info.plist`.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let plist = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("resources/Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    println!(
        "cargo:rustc-link-arg-bin=zwiim-power-helper=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        plist.display()
    );
}
