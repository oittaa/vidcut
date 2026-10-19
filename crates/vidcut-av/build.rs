fn main() {
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" || var("CARGO_CFG_TARGET_ENV") != "msvc" {
        return;
    }
    if let Some(sdk) = std::env::var_os("VULKAN_SDK") {
        let lib = std::path::Path::new(&sdk).join("Lib");
        println!("cargo:rustc-link-search=native={}", lib.display());
    }
    println!("cargo:rustc-link-lib=delayimp");
    println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
    println!("cargo:rustc-link-arg=/IGNORE:4199");
}
