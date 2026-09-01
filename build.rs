// Tell cargo to rebuild whenever the embedded SPA changes.
//
// rust-embed's `Embed` derive reads `frontend/dist` at macro expansion time.
// `cargo:rerun-if-changed` makes cargo re-run *this* build script when the
// directory changes, but cargo only re-expands the proc-macro when the
// source file that hosts the derive looks dirty. So whenever we detect a
// content change in `frontend/dist`, we also bump the mtime on the file
// that contains the `Embed` derive, forcing a re-expansion and a fresh
// embedded bundle.
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;

const EMBED_SOURCE: &str = "src/server/static_files.rs";

fn main() {
    let dist = Path::new("frontend/dist");
    println!("cargo:rerun-if-changed=frontend/dist");
    println!("cargo:rerun-if-changed=build.rs");

    if !dist.is_dir() {
        return;
    }

    let mut hasher = DefaultHasher::new();
    let mut paths = Vec::new();
    walk(dist, &mut paths);
    paths.sort();

    for path in &paths {
        let path_str = path.to_string_lossy();
        println!("cargo:rerun-if-changed={}", path_str);
        path_str.hash(&mut hasher);
        if let Ok(mut f) = fs::File::open(path) {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_ok() {
                buf.hash(&mut hasher);
            }
        }
    }

    let new_hash = hasher.finish();
    let marker = Path::new(&std::env::var("OUT_DIR").unwrap()).join("dist_hash");
    let previous = fs::read_to_string(&marker).ok();
    let new_str = format!("{:x}", new_hash);

    if previous.as_deref() != Some(new_str.as_str()) {
        let _ = fs::write(&marker, &new_str);
        // Bump mtime on the file hosting the rust-embed derive so cargo
        // sees its compile unit as dirty and re-expands the macro.
        if let Ok(content) = fs::read_to_string(EMBED_SOURCE) {
            let _ = fs::write(EMBED_SOURCE, content);
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
}
