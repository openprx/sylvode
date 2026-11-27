use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use serde_json::{Value, json};
use sv_package::{ErrorCode, KeyLookup, Limits, keypair_from_pkcs8, pack, unpack};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn manifest() -> Value {
    json!({
      "schema": "sv.package.v1", "kind": "page-template",
      "id": "sv/sample-pages", "version": "1.0.0",
      "publisher": {"id": "sv", "key_id": "ed25519:0000000000000000"},
      "sv": {"min": "0.6.0", "layout": "sv.layout.v1"},
      "summary": {"en": "Sample"}, "license": "Apache-2.0",
      "binds": {"requests": {"kind": "form", "requires_fields": {"title": "text"}}},
      "layouts": [{"key": "a.b", "path": "contents/layouts/a.json", "slots": ["form.record.detail"]}]
    })
}

fn contents() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("contents/layouts/a.json".to_owned(), b"{\"a\":1}".to_vec())])
}

fn key() -> ring::signature::Ed25519KeyPair {
    let pkcs8 = sv_package::generate_pkcs8().expect("keygen");
    keypair_from_pkcs8(&pkcs8).expect("pair")
}

fn code<T>(r: Result<T, sv_package::PackageError>) -> ErrorCode {
    r.err().expect("must fail").code
}

/// Rewrites a package zip with a mutation applied to the entry list.
fn rewrite(bytes: &[u8], f: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) -> Vec<u8> {
    let mut ar = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut entries = Vec::new();
    for i in 0..ar.len() {
        let mut e = ar.by_index(i).expect("entry");
        let mut d = Vec::new();
        std::io::copy(&mut e, &mut d).expect("read");
        entries.push((e.name().expect("name").to_string(), d));
    }
    f(&mut entries);
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (n, d) in entries {
        w.start_file(n, o).expect("start");
        w.write_all(&d).expect("write");
    }
    w.finish().expect("finish").into_inner()
}

#[test]
fn round_trip_verifies() {
    let k = key();
    let bytes = pack(manifest(), &contents(), &k).expect("pack");
    let pkg = unpack(&bytes, &Limits::default()).expect("unpack");
    let public = sv_package::public_key(&k);
    let v = pkg.clone().verify_with_key(&public).expect("verify");
    assert!(v.digest.starts_with("sha256:"));
    assert_eq!(code(pkg.clone().verify(|_| KeyLookup::Unknown)), ErrorCode::KeyUnknown);
    assert_eq!(code(pkg.verify(|_| KeyLookup::Revoked)), ErrorCode::KeyRevoked);
}

#[test]
fn tampered_content_byte_is_refused() {
    let bytes = pack(manifest(), &contents(), &key()).expect("pack");
    let bad = rewrite(&bytes, |e| {
        if let Some((_, d)) = e.iter_mut().find(|(n, _)| n.starts_with("contents/")) {
            d[0] ^= 1;
        }
    });
    assert_eq!(code(unpack(&bad, &Limits::default())), ErrorCode::DigestMismatch);
}

#[test]
fn tampered_manifest_fails_signature() {
    let k = key();
    let bytes = pack(manifest(), &contents(), &k).expect("pack");
    let bad = rewrite(&bytes, |e| {
        if let Some((_, d)) = e.iter_mut().find(|(n, _)| n == "sv-package.json") {
            let mut v: Value = serde_json::from_slice(d).expect("json");
            v["version"] = json!("1.0.1");
            *d = serde_json::to_vec(&v).expect("ser");
        }
    });
    let pkg = unpack(&bad, &Limits::default()).expect("structure still fine");
    assert_eq!(
        code(pkg.verify_with_key(&sv_package::public_key(&k))),
        ErrorCode::SignatureInvalid
    );
}

#[test]
fn wrong_key_and_garbage_signature_fail() {
    let k = key();
    let bytes = pack(manifest(), &contents(), &k).expect("pack");
    let pkg = unpack(&bytes, &Limits::default()).expect("unpack");
    let other = sv_package::public_key(&key());
    let id = sv_package::key_id(&sv_package::public_key(&k));
    let r = pkg.verify(|got| {
        assert_eq!(got, id);
        KeyLookup::Active(other)
    });
    assert_eq!(code(r), ErrorCode::SignatureInvalid);
    let bad = rewrite(&bytes, |e| {
        if let Some((_, d)) = e.iter_mut().find(|(n, _)| n == "SIGNATURE") {
            *d = b"AAAA\n".to_vec();
        }
    });
    assert_eq!(code(unpack(&bad, &Limits::default())), ErrorCode::SignatureInvalid);
}

#[test]
fn path_traversal_and_unlisted_files_are_refused() {
    let bytes = pack(manifest(), &contents(), &key()).expect("pack");
    for name in ["contents/../evil", "/abs", "contents\\x", "C:/x", "contents//x"] {
        let bad = rewrite(&bytes, |e| e.push((name.to_owned(), b"x".to_vec())));
        assert_eq!(code(unpack(&bad, &Limits::default())), ErrorCode::PathInvalid, "{name}");
    }
    let extra = rewrite(&bytes, |e| e.push(("contents/extra.json".into(), b"x".to_vec())));
    assert_eq!(code(unpack(&extra, &Limits::default())), ErrorCode::FileUnlisted);
    let missing = rewrite(&bytes, |e| e.retain(|(n, _)| !n.starts_with("contents/")));
    assert_eq!(code(unpack(&missing, &Limits::default())), ErrorCode::FileMissing);
    let dup = rewrite(&bytes, |e| e.push(("CONTENTS/layouts/A.json".into(), b"x".to_vec())));
    assert_eq!(code(unpack(&dup, &Limits::default())), ErrorCode::PathInvalid);
}

#[test]
fn limits_and_manifest_rules() {
    let bytes = pack(manifest(), &contents(), &key()).expect("pack");
    let tiny = Limits {
        max_entries: 2,
        ..Limits::default()
    };
    assert_eq!(code(unpack(&bytes, &tiny)), ErrorCode::TooManyEntries);
    let small = Limits {
        max_entry_bytes: 3,
        ..Limits::default()
    };
    assert_eq!(code(unpack(&bytes, &small)), ErrorCode::TooLarge);
    let mut m = manifest();
    m["requests"] = json!(["forms.records.read@form=$bind.requests"]);
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
    let mut m = manifest();
    m["id"] = json!("other/sample-pages");
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
    let mut m = manifest();
    m["version"] = json!("one");
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
}

#[test]
fn app_requests_resolve_binds() {
    let mut m = manifest();
    m["kind"] = json!("app");
    m["components"] = json!([{"world": "action", "path": "contents/c.wasm", "exports": ["x"]}]);
    m["requests"] = json!([
        "forms.records.read@form=$bind.requests",
        "forms.records.update@form=$install"
    ]);
    let mut c = contents();
    c.insert("contents/c.wasm".into(), vec![0, 1]);
    pack(m.clone(), &c, &key()).expect("valid app");
    m["requests"] = json!(["forms.records.read@form=$bind.nope"]);
    assert_eq!(code(pack(m, &c, &key())), ErrorCode::ManifestInvalid);
}

#[test]
fn duplicate_keys_floats_and_custom_code_are_refused() {
    assert_eq!(
        code(sv_package::parse_manifest(br#"{"a":1,"a":2}"#)),
        ErrorCode::ManifestInvalid
    );
    assert_eq!(
        code(sv_package::parse_manifest(b"{\"a\":1.5}")),
        ErrorCode::ManifestInvalid
    );
    assert_eq!(
        code(sv_package::parse_manifest(b"{\"a\":9007199254740993}")),
        ErrorCode::ManifestInvalid
    );
    assert!(sv_package::parse_manifest(br#"{"a":[1,{"b":null}]}"#).is_ok());
    let mut m = manifest();
    m["custom_code"] = json!(true);
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
    let mut m = manifest();
    m["extra"] = json!(1);
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
}
