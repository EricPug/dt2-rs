fn main() {
    println!("cargo:rerun-if-env-changed=UNICORN_LIB_DIR");
    if std::env::var_os("CARGO_FEATURE_UNICORN").is_some() {
        let dir = std::env::var("UNICORN_LIB_DIR").expect("UNICORN_LIB_DIR must point at the patched libunicorn");
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=dylib=unicorn");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
