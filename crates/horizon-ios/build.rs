//! iOS gives the main thread 1 MB of stack and the game's startup overflows it (`Renderer::new`):
//! the app asks for 16 MB, more than the 8 MB main thread the desktop systems give.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        println!("cargo:rustc-link-arg-bins=-Wl,-stack_size,0x1000000");
    }
}
