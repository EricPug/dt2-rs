use std::path::PathBuf;

// The Unicorn tier is built when the patched libunicorn is found (UNICORN_LIB_DIR, or the
// default output of tools/unicorn/build.sh, ~/.cache/dt2-unicorn); otherwise dt2-cli is the pure-Rust interpreter.
// `--features unicorn` makes a missing library a build error instead of a silent fallback.
fn main() {
    println!("cargo:rerun-if-env-changed=UNICORN_LIB_DIR");
    println!("cargo:rustc-check-cfg=cfg(dt2_unicorn)");
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_default();
    let default_dir = cache.join("dt2-unicorn/build");
    println!("cargo:rerun-if-changed={}", default_dir.join("libunicorn.so").display());
    let dir = std::env::var_os("UNICORN_LIB_DIR")
        .map(PathBuf::from)
        .or_else(|| default_dir.join("libunicorn.so").exists().then(|| default_dir.clone()));
    match dir {
        Some(dir) => {
            let dir = dir.canonicalize().unwrap_or(dir);
            println!("cargo:rustc-cfg=dt2_unicorn");
            println!("cargo:rustc-link-search=native={}", dir.display());
            println!("cargo:rustc-link-lib=dylib=unicorn");
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        None if std::env::var_os("CARGO_FEATURE_UNICORN").is_some() => {
            panic!("feature `unicorn` needs the patched libunicorn: run tools/unicorn/build.sh or set UNICORN_LIB_DIR")
        }
        None => println!("cargo:warning=patched libunicorn not found; building the Rust interpreter only"),
    }
}
