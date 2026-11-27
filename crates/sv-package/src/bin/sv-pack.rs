//! `sv-pack`: build, sign and inspect `sv.package.v1` packages.
//!
//! ```text
//! sv-pack keygen  <key-file> <public-key-file>
//! sv-pack pack    <dir> <key-file> <out.svpkg>
//! sv-pack inspect <package.svpkg>
//! ```
//!
//! `<dir>` holds `sv-package.json` and a `contents/` tree. The key file is PKCS#8 and secret: it
//! is written with mode 0600 and never printed.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sv_package::{Limits, MANIFEST_NAME, PackageError};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match refs.as_slice() {
        ["keygen", key, public] => keygen(key, public),
        ["pack", dir, key, out] => pack(dir, key, out),
        ["inspect", pkg] => inspect(pkg),
        _ => Err(
            "usage: sv-pack keygen <key-file> <public-key-file> | pack <dir> <key-file> <out> | inspect <package>"
                .into(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sv-pack: {e}");
            ExitCode::FAILURE
        }
    }
}

fn io<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> String + '_ {
    move |e| format!("{what}: {e}")
}

fn pe(e: PackageError) -> String {
    e.to_string()
}

fn keygen(key_path: &str, public_path: &str) -> Result<(), String> {
    let pkcs8 = sv_package::generate_pkcs8().map_err(pe)?;
    let pair = sv_package::keypair_from_pkcs8(&pkcs8).map_err(pe)?;
    let public = sv_package::public_key(&pair);
    write_secret(Path::new(key_path), &pkcs8)?;
    fs::write(public_path, format!("{}\n", URL_SAFE_NO_PAD.encode(&public))).map_err(io("write public key"))?;
    println!("key_id {}", sv_package::key_id(&public));
    Ok(())
}

fn write_secret(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(io("create key file"))?;
    f.write_all(data).map_err(io("write key file"))
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(io("read dir"))? {
        let entry = entry.map_err(io("read dir"))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io("file type"))?;
        if kind.is_symlink() {
            return Err(format!("symlink refused: {}", path.display()));
        }
        if kind.is_dir() {
            collect(root, &path, out)?;
        } else if kind.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(io("path"))?
                .to_str()
                .ok_or("non-UTF-8 path")?
                .replace('\\', "/");
            out.insert(rel, fs::read(&path).map_err(io("read file"))?);
        }
    }
    Ok(())
}

fn pack(dir: &str, key_path: &str, out: &str) -> Result<(), String> {
    let root = Path::new(dir);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(MANIFEST_NAME)).map_err(io("read manifest"))?)
            .map_err(io("manifest JSON"))?;
    let mut contents = BTreeMap::new();
    let contents_dir = root.join("contents");
    if contents_dir.is_dir() {
        collect(root, &contents_dir, &mut contents)?;
    }
    let pair = sv_package::keypair_from_pkcs8(&fs::read(key_path).map_err(io("read key"))?).map_err(pe)?;
    let bytes = sv_package::pack(manifest, &contents, &pair).map_err(pe)?;
    fs::write(out, &bytes).map_err(io("write package"))?;
    let pkg = sv_package::unpack(&bytes, &Limits::default()).map_err(pe)?;
    println!("digest {}", pkg.digest);
    println!("blob_digest {}", pkg.blob_digest);
    Ok(())
}

fn inspect(path: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(io("read package"))?;
    let pkg = sv_package::unpack(&bytes, &Limits::default()).map_err(pe)?;
    println!("id {}", pkg.manifest.id);
    println!("version {}", pkg.manifest.version);
    println!("key_id {}", pkg.manifest.publisher.key_id);
    println!("digest {}", pkg.digest);
    println!("blob_digest {}", pkg.blob_digest);
    println!("files {}", pkg.files.len());
    println!("signature not verified (no key registry here)");
    Ok(())
}
