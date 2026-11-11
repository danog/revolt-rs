//! The extension's Zend symbols are resolved from the php binary when it is loaded (macOS needs
//! that said explicitly). On Linux, -Bsymbolic binds this copy's references to its own symbols
//! (`revolt_rs_abi_v1`), whatever other copy of revolt-rs is loaded with RTLD_GLOBAL.
fn main() {
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => {
            println!("cargo:rustc-cdylib-link-arg=-undefined");
            println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
            // the system libraries Rust's std links come from the SDK, even when LIBRARY_PATH
            // points at a package manager's copies (e.g. Homebrew's GNU libiconv)
            if let Ok(o) = std::process::Command::new("xcrun").arg("--show-sdk-path").output() {
                let sdk = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                if o.status.success() && !sdk.is_empty() {
                    println!("cargo:rustc-link-search=native={sdk}/usr/lib");
                }
            }
        }
        Ok("linux") | Ok("freebsd") => println!("cargo:rustc-cdylib-link-arg=-Wl,-Bsymbolic"),
        _ => {}
    }
}
