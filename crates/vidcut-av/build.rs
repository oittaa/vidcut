use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const VAD_FILE: &str = "silero-vad-v6.2-F32.gguf";
const VAD_URL: &str = "https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/a9b2d61c20ae4ddf3644edb34969e8185a485448/silero-vad-v6.2-F32.gguf";
const VAD_SHA256: &str = "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    println!("cargo:rerun-if-env-changed=VIDCUT_SILERO_VAD");
    vad();
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" || var("CARGO_CFG_TARGET_ENV") != "msvc" {
        return;
    }
    if let Some(sdk) = std::env::var_os("VULKAN_SDK") {
        let lib = Path::new(&sdk).join("Lib");
        println!("cargo:rustc-link-search=native={}", lib.display());
    }
    println!("cargo:rustc-link-lib=delayimp");
    println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
    println!("cargo:rustc-link-arg=/IGNORE:4199");
}

fn sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

fn vad() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join(VAD_FILE);
    if sha256(&out).as_deref() == Some(VAD_SHA256) {
        return;
    }
    let part = out.with_extension("part");
    let _ = std::fs::remove_file(&part);
    let how = match std::env::var_os("VIDCUT_SILERO_VAD") {
        Some(given) => {
            if let Err(e) = std::fs::copy(&given, &part) {
                panic!("VIDCUT_SILERO_VAD={}: {e}", Path::new(&given).display());
            }
            format!("VIDCUT_SILERO_VAD={}", Path::new(&given).display())
        }
        None => {
            let script = out.with_extension("cmake");
            std::fs::write(
                &script,
                "file(DOWNLOAD \"${URL}\" \"${DEST}\" EXPECTED_HASH SHA256=${SHA} TLS_VERIFY ON STATUS s)\n\
                 list(GET s 0 code)\n\
                 if(NOT code EQUAL 0)\n  message(FATAL_ERROR \"${s}\")\nendif()\n",
            )
            .expect("writing the download script");
            let cmake = std::env::var_os("CMAKE").unwrap_or_else(|| "cmake".into());
            let ran = std::process::Command::new(&cmake)
                .arg(format!("-DURL={VAD_URL}"))
                .arg(format!("-DDEST={}", part.display()))
                .arg(format!("-DSHA={VAD_SHA256}"))
                .arg("-P")
                .arg(&script)
                .status();
            if !ran.as_ref().is_ok_and(|s| s.success()) {
                let _ = std::fs::remove_file(&part);
                panic!(
                    "downloading the Silero VAD model with CMake failed ({ran:?}): download \
                     {VAD_URL} and set VIDCUT_SILERO_VAD to the file"
                );
            }
            VAD_URL.to_string()
        }
    };
    let got = sha256(&part);
    if got.as_deref() != Some(VAD_SHA256) {
        let _ = std::fs::remove_file(&part);
        panic!("{how}: SHA-256 {got:?}, expected {VAD_SHA256}");
    }
    std::fs::rename(&part, &out).expect("keeping the Silero VAD model");
}
