use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (out.status.success() && !text.is_empty()).then(|| text.to_string())
}

fn build() -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let commit = git(&[
        "describe",
        "--always",
        "--dirty",
        "--abbrev=8",
        "--exclude=*",
    ]);
    let date = git(&["log", "-1", "--format=%cd", "--date=format:%Y%m%d"]);
    let (Some(commit), Some(date)) = (commit, date) else {
        return version;
    };
    match commit.strip_suffix("-dirty") {
        Some(hash) => format!("{version}-g{hash}-{date}-dirty"),
        None => format!("{version}-g{commit}-{date}"),
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for sources in ["src", "../vidcut-core/src", "../vidcut-av/src"] {
        println!("cargo:rerun-if-changed={sources}");
    }
    let head = git(&["symbolic-ref", "-q", "HEAD"]);
    let files = ["HEAD", "packed-refs"].into_iter().map(String::from);
    for file in files.chain(head) {
        if let Some(path) = git(&["rev-parse", "--git-path", &file]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rustc-env=VIDCUT_BUILD={}", build());
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") == "windows" && var("CARGO_CFG_TARGET_ENV") == "msvc" {
        println!("cargo:rustc-link-arg=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg=/IGNORE:4199");
    }
}
