//! Links the app icon (assets/dynamic-notch.ico) into the exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/dynamic-notch.rc");
    println!("cargo:rerun-if-changed=assets/dynamic-notch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/dynamic-notch.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile Windows resources");
    }
}
