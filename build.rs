use std::fs;
use std::path::Path;
use std::env;

fn main() {
    // Tell Cargo to rerun this script if these files change
    println!("cargo:rerun-if-changed=config.toml");
    println!("cargo:rerun-if-changed=ControlMyMonitor.exe");

    // Get the output directory (target/debug or target/release)
    let profile = env::var("PROFILE").unwrap();
    let target_dir = Path::new("target").join(profile);

    // List of files to bundle with your app
    let files_to_copy = ["config.toml", "ControlMyMonitor.exe", "icon.ico"];

    for filename in files_to_copy {
        let src = Path::new(filename);
        let dest = target_dir.join(filename);

        if src.exists() {
            if let Err(e) = fs::copy(src, &dest) {
                // This will show up in the terminal if the copy fails
                println!("cargo:warning=Failed to copy {}: {}", filename, e);
            } else {
                println!("cargo:info=Successfully copied {} to {}", filename, dest.display());
            }
        } else {
            println!("cargo:warning=Source file {} not found in project root!", filename);
        }
    }
}