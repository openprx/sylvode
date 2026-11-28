#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use serde_json::{Value, json};
use sv_package::{ErrorCode, KeyLookup, Limits, keypair_from_pkcs8, pack, unpack};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn manifest() -> Value {
    json!({
      "schema": "sv.package.v1", "kind": "page-template",
      "id": "sv/sample-pages", "version": "1.0.0",
      "publisher": {"id": "sv", "key_id": "ed25519:00000000000000000000000000000000"},
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
    m["requests"] = json!([{"id": "forms.records.read", "form": "$bind.requests"}]);
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
    let mut m = manifest();
    m["id"] = json!("other/sample-pages");
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
    let mut m = manifest();
    m["version"] = json!("one");
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
}

fn app_manifest() -> (Value, BTreeMap<String, Vec<u8>>) {
    let mut m = manifest();
    m["kind"] = json!("app");
    m["wit"] = json!("sv:plugin@4.0.0");
    m["actions"] = json!([{"name": "close_request", "world": "action", "component": "contents/c.wasm",
                           "label": {"en": "Close"}}]);
    m["rules"] = json!([{"name": "ref_code", "world": "field-rule", "component": "contents/r.wasm",
                         "stages": ["validate"], "field_types": ["text"]}]);
    m["requests"] = json!([
        {"id": "forms.records.read", "form": "$bind.requests"},
        {"id": "forms.records.update"}
    ]);
    m["messages"] = json!({"bad_ref": {"en": "Bad reference"}});
    let mut c = contents();
    c.insert("contents/c.wasm".into(), vec![0, 1]);
    c.insert("contents/r.wasm".into(), vec![0, 2]);
    (m, c)
}

#[test]
fn app_manifest_fields_are_checked() {
    let (m, c) = app_manifest();
    let bytes = pack(m.clone(), &c, &key()).expect("valid app");
    let pkg = unpack(&bytes, &Limits::default()).expect("unpack");
    assert!(pkg.manifest.computed_custom_code());
    let broken = |f: &dyn Fn(&mut Value)| {
        let mut v = m.clone();
        f(&mut v);
        code(pack(v, &c, &key()))
    };
    let bad = ErrorCode::ManifestInvalid;
    assert_eq!(broken(&|v| v["requests"][0]["form"] = json!("$bind.nope")), bad);
    assert_eq!(broken(&|v| v["requests"][1]["id"] = json!("forms.*")), bad);
    assert_eq!(broken(&|v| v["wit"] = json!("sv:plugin@3.0.0")), bad);
    assert_eq!(
        broken(&|v| {
            v.as_object_mut().map(|o| o.remove("wit"));
        }),
        bad
    );
    assert_eq!(broken(&|v| v["rules"][0]["component"] = json!("contents/c.wasm")), bad);
    assert_eq!(broken(&|v| v["actions"][0]["world"] = json!("job")), bad);
    assert_eq!(broken(&|v| v["rules"][0]["stages"] = json!(["bogus"])), bad);
    assert_eq!(broken(&|v| v["messages"] = json!({"Bad-Code": {"en": "x"}})), bad);
    assert_eq!(broken(&|v| v["actions"][0]["name"] = json!("Close")), bad);
    assert_eq!(
        broken(&|v| v["pages"] = json!([{"route": "/x", "layout": "nope", "audience": "visitor"}])),
        bad
    );
    assert_eq!(broken(&|v| v["custom_code"] = json!(false)), bad);
    // A plugin may not carry pages; a form template may not carry actions.
    assert_eq!(
        broken(&|v| {
            v["kind"] = json!("plugin");
            v["pages"] = json!([{"route": "/x", "layout": "a.b", "audience": "visitor"}]);
        }),
        bad
    );
    assert_eq!(broken(&|v| v["kind"] = json!("form-template")), bad);
}

#[test]
fn key_id_has_128_bits() {
    let id = sv_package::key_id(&sv_package::public_key(&key()));
    assert_eq!(id.len(), "ed25519:".len() + 32);
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
    for bad in [
        "{\"a\":-9223372036854775808}",
        "{\"a\":9007199254740993}",
        "{\"a\":-9007199254740993}",
        "{\"a\":\"x\\u0000y\"}",
        "{\"a\\u0000\":1}",
    ] {
        assert_eq!(
            code(sv_package::parse_manifest(bad.as_bytes())),
            ErrorCode::ManifestInvalid,
            "{bad}"
        );
    }
    assert!(sv_package::parse_manifest(b"{\"a\":9007199254740992,\"b\":-9007199254740992}").is_ok());
    assert!(sv_package::parse_manifest(br#"{"a":[1,{"b":null}]}"#).is_ok());
    let mut m = manifest();
    m["custom_code"] = json!(true);
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
    let mut m = manifest();
    m["extra"] = json!(1);
    assert_eq!(code(pack(m, &contents(), &key())), ErrorCode::ManifestInvalid);
}

/// A package zip with the bytes of a deliberately inconsistent container.
fn good_zip() -> Vec<u8> {
    pack(manifest(), &contents(), &key()).expect("pack")
}

#[test]
fn zip_with_leading_or_trailing_data_is_refused() {
    let good = good_zip();
    unpack(&good, &Limits::default()).expect("baseline");
    let mut trailing = good.clone();
    trailing.extend_from_slice(b"junk");
    assert_eq!(code(unpack(&trailing, &Limits::default())), ErrorCode::ZipInvalid);
    let mut leading = b"junk".to_vec();
    leading.extend_from_slice(&good);
    assert_eq!(code(unpack(&leading, &Limits::default())), ErrorCode::ZipInvalid);
    assert_eq!(
        code(unpack(&good[..good.len() - 5], &Limits::default())),
        ErrorCode::ZipInvalid
    );
}

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("pattern present")
}

#[test]
fn local_header_must_match_the_central_directory() {
    let good = good_zip();
    // The first local header's CRC field differs from the central entry's.
    let mut crc = good.clone();
    crc[14] ^= 0xff;
    assert_eq!(code(unpack(&crc, &Limits::default())), ErrorCode::ZipInvalid);
    // The first local header names another file of the same length.
    let mut name = good.clone();
    let at = find(&name, b"sv-package.json");
    name[at] = b'x';
    assert_eq!(code(unpack(&name, &Limits::default())), ErrorCode::ZipInvalid);
    // A gap between entries: shift the central directory offset past extra bytes.
    let mut gap = good;
    let eocd = find(&gap, &[0x50, 0x4b, 0x05, 0x06]);
    let cd = u32::from_le_bytes(gap[eocd + 16..eocd + 20].try_into().expect("4 bytes"));
    gap.splice(cd as usize..cd as usize, *b"xxxx");
    let eocd = find(&gap, &[0x50, 0x4b, 0x05, 0x06]);
    gap[eocd + 16..eocd + 20].copy_from_slice(&(cd + 4).to_le_bytes());
    assert_eq!(code(unpack(&gap, &Limits::default())), ErrorCode::ZipInvalid);
}

#[test]
fn eocd_comment_and_zip64_markers_are_refused() {
    let good = good_zip();
    let mut comment = good.clone();
    let eocd = find(&comment, &[0x50, 0x4b, 0x05, 0x06]);
    comment[eocd + 20] = 3;
    comment.extend_from_slice(b"abc");
    assert_eq!(code(unpack(&comment, &Limits::default())), ErrorCode::ZipInvalid);
    let mut z64 = good;
    let eocd = find(&z64, &[0x50, 0x4b, 0x05, 0x06]);
    z64[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(code(unpack(&z64, &Limits::default())), ErrorCode::ZipInvalid);
}

fn template_manifest(project: &Value) -> Value {
    json!({
      "schema": "sv.package.v1", "kind": "form-template",
      "id": "sv/sample-form", "version": "1.0.0",
      "publisher": {"id": "sv", "key_id": "ed25519:00000000000000000000000000000000"},
      "sv": {"min": "0.6.0"},
      "summary": {"en": "Sample"}, "license": "Apache-2.0",
      "binds": {"main": {"kind": "form", "create_from": "contents/forms/main.json"}},
      "project": project
    })
}

fn template_contents() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("contents/forms/main.json".to_owned(), b"{\"key\":\"main\"}".to_vec())])
}

fn states() -> Value {
    json!([
      {"key": "draft", "name": {"en": "Draft"}, "category": "unstarted", "initial": true},
      {"key": "done", "name": {"en": "Done", "zh": "完成"}, "category": "completed", "color": "#16a34a"}
    ])
}

fn packs(project: &Value) -> bool {
    pack(template_manifest(project), &template_contents(), &key()).is_ok()
}

#[test]
fn a_project_block_in_range_packs() {
    assert!(packs(&json!({
      "type_key": "custom_form", "workflow_states": states(),
      "labels": [{"name": {"en": "Risk", "zh": "风险"}, "color": "#dc2626"}]
    })));
    assert!(packs(&json!({"type_key": "custom_form"})));
}

#[test]
fn a_project_block_out_of_range_is_refused() {
    let with_states = |states: Value| json!({"workflow_states": states});
    // Unknown member, type grammar, upper-case colours, one initial state, one completed state.
    assert!(!packs(&json!({"governance": {}})));
    assert!(!packs(&json!({"type_key": "Custom-Form"})));
    assert!(!packs(
        &json!({"labels": [{"name": {"en": "Risk"}, "color": "#DC2626"}]})
    ));
    assert!(!packs(&json!({"labels": [{"name": {"en": "Risk"}, "color": "red"}]})));
    assert!(!packs(
        &json!({"labels": [{"name": {"en": "Risk"}, "color": "#dc2626"}, {"name": {"en": "RISK"}, "color": "#dc2626"}]})
    ));
    assert!(!packs(
        &json!({"labels": [{"name": {"zh": "风险"}, "color": "#dc2626"}]})
    ));
    assert!(!packs(&with_states(json!([states()[0].clone()]))));
    let mut two_initial = states();
    two_initial[1]["initial"] = json!(true);
    assert!(!packs(&with_states(two_initial)));
    let mut none_completed = states();
    none_completed[1]["category"] = json!("active");
    assert!(!packs(&with_states(none_completed)));
    let mut bad_category = states();
    bad_category[1]["category"] = json!("done");
    assert!(!packs(&with_states(bad_category)));
    let mut padded = states();
    padded[0]["name"]["en"] = json!(" Draft");
    assert!(!packs(&with_states(padded)));
    let mut bidi = states();
    bidi[0]["name"]["en"] = json!("Dra\u{202e}ft");
    assert!(!packs(&with_states(bidi)));
    let mut invisible = states();
    invisible[0]["name"]["en"] = json!("Dra\u{200b}ft");
    assert!(!packs(&with_states(invisible)));
    let mut soft_hyphen = states();
    soft_hyphen[0]["name"]["en"] = json!("Dra\u{ad}ft");
    assert!(!packs(&with_states(soft_hyphen)));
    let mut repeated = states();
    repeated[1]["key"] = json!("draft");
    assert!(!packs(&with_states(repeated)));
}

#[test]
fn a_project_block_is_refused_outside_form_templates_and_apps() {
    let mut m = manifest();
    m["project"] = json!({"type_key": "custom_form"});
    assert!(pack(m, &contents(), &key()).is_err());
}
