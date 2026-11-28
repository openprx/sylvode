//! Small checks of the validator, the image sniffer and the comparison.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use super::*;

fn sample() -> Value {
    let public = [7u8; 32];
    let key_id = sv_package::key_id(&public);
    let version = json!({
        "version": "1.4.0", "published_at": 1_795_000_000u64, "status": "listed",
        "url": "https://github.com/acme/pricing/releases/download/v1.4.0/acme-pricing-1.4.0.svpkg",
        "size": 123_456, "sha256": "a".repeat(64), "digest": format!("sha256:{}", "b".repeat(64)),
        "publisher": "acme", "key_id": key_id, "sylvode": ">=0.6.0, <0.8.0",
        "contents": {"requests": [], "custom_code": false, "actions": [], "rules": [], "binds": [],
                     "layouts": [], "pages": [], "forms": [], "project_block": false}
    });
    json!({
        "schema": "sv.plugin-index.v1", "generated_at": 1_795_000_001u64, "title": {"en": "Official"},
        "metadata": {"categories": {"finance": {"title": {"en": "Finance"}}}, "labels": {}},
        "publishers": {"acme": {
            "schema": "sv.plugin-index.publisher.v1", "id": "acme", "name": {"en": "Acme Labs"},
            "maintainers": ["acme-dev"],
            "keys": [{"key_id": key_id, "public_key": URL_SAFE_NO_PAD.encode(public),
                      "status": "active", "added_at": 1_790_000_000u64}]}},
        "plugins": [{
            "schema": "sv.plugin-index.plugin.v1", "id": "acme/pricing", "kind": "plugin",
            "title": {"en": "Pricing"}, "summary": {"en": "Prices"}, "description": {"en": "Line one\n\nLine two"},
            "icon": "icon.png", "license": "MIT", "metadata": {"category": "finance"},
            "channels": {"latest": "1.4.0"}, "versions": [version]}]
    })
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

fn paths(e: &ValidationError) -> Vec<&str> {
    e.issues.iter().map(|i| i.path.as_str()).collect()
}

#[test]
fn valid_index_passes_both_modes() {
    let b = bytes(&sample());
    assert!(parse_index(&b, &Options::strict()).is_ok());
    assert!(parse_index(&b, &Options::lenient()).is_ok());
}

#[test]
fn unknown_member_is_strict_only() {
    let mut v = sample();
    v["plugins"][0]["versions"][0]["surprise"] = json!(1);
    let b = bytes(&v);
    let e = parse_index(&b, &Options::strict()).unwrap_err();
    assert_eq!(paths(&e), ["$.plugins[0].versions[0].surprise"]);
    assert!(parse_index(&b, &Options::lenient()).is_ok());
}

#[test]
fn violations_carry_json_paths() {
    let mut v = sample();
    v["plugins"][0]["versions"][0]["sha256"] = json!("xyz");
    v["plugins"][0]["versions"][0]["url"] = json!("http://example.com/a.svpkg");
    v["plugins"][0]["title"] = json!({"de": "Preise"});
    let e = parse_index(&bytes(&v), &Options::lenient()).unwrap_err();
    let p = paths(&e);
    assert!(p.contains(&"$.plugins[0].versions[0].sha256"));
    assert!(p.contains(&"$.plugins[0].versions[0].url"));
    assert!(p.contains(&"$.plugins[0].title"));
}

#[test]
fn type_errors_carry_json_paths() {
    let mut v = sample();
    v["plugins"][0]["versions"][0]["size"] = json!("big");
    let e = parse_index(&bytes(&v), &Options::lenient()).unwrap_err();
    assert_eq!(paths(&e), ["$.plugins[0].versions[0].size"]);
}

#[test]
fn download_urls() {
    let any = AnyHost;
    assert!(download_url_problem("https://example.com/a.svpkg", &any).is_none());
    for bad in [
        "http://example.com/a",
        "https://127.0.0.1/a",
        "https://0x7f.1/a",
        "https://[::1]/a",
        "https://u:p@example.com/a",
        "https://example.com:8443/a",
        "https://example.com/a?x=1",
        "https://example.com/a#f",
        "https://exa\nmple.com/a",
    ] {
        assert!(download_url_problem(bad, &any).is_some(), "{bad}");
    }
    let only = ExactHosts(vec!["github.com".to_owned()]);
    assert!(download_url_problem("https://github.com/a", &only).is_none());
    assert!(download_url_problem("https://example.com/a", &only).is_some());
}

#[test]
fn compat_range_floor() {
    let low = |s: &str| lowest_admitted(&parse_compat(s).unwrap()).map(|v| v.to_string());
    assert_eq!(low(">=0.6.0, <0.8.0").as_deref(), Some("0.6.0"));
    assert_eq!(low(">0.6").as_deref(), Some("0.7.0"));
    assert_eq!(low("^1.2").as_deref(), Some("1.2.0"));
    assert!(parse_compat(">=2, <1").is_err());
}

#[test]
fn image_headers() {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&128u32.to_be_bytes());
    png.extend_from_slice(&128u32.to_be_bytes());
    assert_eq!(sniff(&png).unwrap().width, 128);
    assert!(check_image(ImageRole::Icon, Some("icon.png"), &png).is_ok());
    assert!(check_image(ImageRole::Icon, Some("icon.webp"), &png).is_err());
    assert!(check_image(ImageRole::Screenshot, None, &png).is_err());

    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 11, 8, 1, 0x90, 2, 0x58, 1, 1, 0, 0,
    ];
    let info = sniff(&jpeg).unwrap();
    assert_eq!((info.width, info.height), (600, 400));

    let mut webp = b"RIFF\0\0\0\0WEBPVP8X\n\0\0\0\0\0\0\0".to_vec();
    webp.extend_from_slice(&[0x7F, 0x01, 0x00, 0x8F, 0x00, 0x00]);
    let info = sniff(&webp).unwrap();
    assert_eq!((info.width, info.height), (384, 144));

    assert_eq!(
        sniff(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap_err().0,
        "SVG is not allowed"
    );
    assert!(sniff(b"GIF89a").is_err());
}

fn parsed() -> IndexDoc {
    parse_index(&bytes(&sample()), &Options::strict()).unwrap()
}

#[test]
fn append_only_and_one_way_status() {
    let base = parsed();

    let mut head = base.clone();
    head.plugins[0].versions[0].status = VersionStatus::Yanked;
    head.plugins[0].versions[0].status_reason = Some("broken".to_owned());
    let c = compare(&base, &head);
    assert!(c.issues.is_empty());
    assert_eq!(c.changes.status_changes.len(), 1);

    let c = compare(&head, &base);
    assert_eq!(c.issues.len(), 1);
    assert_eq!(c.issues[0].path, "$.plugins[0].versions[0].status");

    let mut edited = base.clone();
    edited.plugins[0].versions[0].sha256 = "c".repeat(64);
    assert_eq!(compare(&base, &edited).issues.len(), 1);

    let mut removed = base.clone();
    removed.plugins[0].versions.clear();
    assert!(!compare(&base, &removed).issues.is_empty());

    let mut gone = base.clone();
    gone.plugins.clear();
    gone.publishers.clear();
    assert_eq!(compare(&base, &gone).issues.len(), 2);

    let mut revoked = base.clone();
    if let Some(p) = revoked.publishers.get_mut("acme") {
        p.keys[0].status = KeyStatus::Revoked;
        p.keys[0].revoked_at = Some(1_795_000_000);
        p.keys[0].reason = Some("compromised".to_owned());
    }
    let c = compare(&base, &revoked);
    assert!(c.issues.is_empty());
    assert_eq!(c.changes.revoked_keys.len(), 1);
    assert!(!compare(&revoked, &base).issues.is_empty());
}
