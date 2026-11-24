fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Disable high-entropy ASLR so title allocations stay below 2^40 (required by XboxBvhBuilder).
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bin=xclvic=/HIGHENTROPYVA:NO"),
        Ok("gnu") => println!("cargo:rustc-link-arg-bin=xclvic=-Wl,--disable-high-entropy-va"),
        _ => {}
    }
}
