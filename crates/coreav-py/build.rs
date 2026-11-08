//! Recognized CLI-only configuration in the embedded command source.

fn main() {
    println!("cargo:rustc-check-cfg=cfg(feature, values(\"native-h264-encoder\"))");
}
