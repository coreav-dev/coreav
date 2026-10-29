//! Links the system's libvpx with the `libvpx` feature, as `pkg-config`
//! describes it.

fn main() {
    #[cfg(feature = "libvpx")]
    if let Err(e) = pkg_config::Config::new().probe("vpx") {
        println!("cargo::error=the libvpx feature needs libvpx, found with pkg-config: {e}");
    }
}
