use super::*;
use crate::package::generate_signing_key;
use serde_json::{json, Value};
const NOW: u64 = 1_000_000_000;
const HOST: &str = "0.1.0-alpha.1";

fn fixture() -> (Config, [u8; 32], Value) {
    let (secret, key) = generate_signing_key().unwrap();
    let config = Config {
        origin: "https://plugins.example.org".into(),
        keys: vec![key.clone()],
        revoked: vec![],
        minimum_sequence: 1,
    };
    let v = json!({"version":"0.1.0","api":API_VERSION,"url":format!("{}/v1/packages/notes/0.1.0/{}.terrapkg",config.origin,"a".repeat(64)),"size":123,"digest":"a".repeat(64),"capabilities":[],"min_host":HOST,"yanked":false});
    let value = json!({"format":1,"registry":REGISTRY_ID,"generated_ms":NOW,"expires_ms":NOW+DAY,"sequence":2,"plugins":[{"id":"notes","name":"Notes","description":"Synthetic","icon":"inbox","publisher":key,"versions":[v]}]});
    (config, secret, value)
}
fn raw_signature(bytes: &[u8], secret: &[u8; 32]) -> Vec<u8> {
    let key = SigningKey::from_bytes(secret);
    serde_json::to_vec(&Detached {
        format: 1,
        key: B64.encode(key.verifying_key().as_bytes()),
        sig: B64.encode(key.sign(message(bytes).as_bytes()).to_bytes()),
    })
    .unwrap()
}
fn checked(
    value: &Value,
    secret: &[u8; 32],
    config: &Config,
    last: &HighWater,
    now: u64,
) -> Result<Index> {
    let bytes = serde_json::to_vec(value).unwrap();
    verify(&bytes, &raw_signature(&bytes, secret), config, last, now)
}
#[test]
fn valid_index_verifies() {
    let (c, s, v) = fixture();
    let bytes = serde_json::to_vec(&v).unwrap();
    let sig = sign_index(&bytes, &s, &c).unwrap();
    assert!(verify(&bytes, &sig, &c, &HighWater::default(), NOW).is_ok());
    let mut empty = v;
    empty["plugins"] = json!([]);
    assert!(checked(&empty, &s, &c, &HighWater::default(), NOW).is_ok());
}
#[test]
fn bad_signature_fails() {
    let (mut c, s, v) = fixture();
    let bytes = serde_json::to_vec(&v).unwrap();
    let sig = raw_signature(&bytes, &s);
    let mut changed = bytes.clone();
    changed.push(b' ');
    assert!(verify(&changed, &sig, &c, &HighWater::default(), NOW).is_err());
    let (other, _) = generate_signing_key().unwrap();
    assert!(verify(
        &bytes,
        &raw_signature(&bytes, &other),
        &c,
        &HighWater::default(),
        NOW
    )
    .is_err());
    c.revoked = c.keys.clone();
    assert!(verify(&bytes, &sig, &c, &HighWater::default(), NOW).is_err());
    c.revoked.clear();
    c.keys.clear();
    assert!(verify(&bytes, &sig, &c, &HighWater::default(), NOW).is_err());
}
#[test]
fn signature_domain_encoding_and_shape_are_strict() {
    let (c, s, v) = fixture();
    let bytes = serde_json::to_vec(&v).unwrap();
    let key = SigningKey::from_bytes(&s);
    let mut sig: Value = serde_json::from_slice(&raw_signature(&bytes, &s)).unwrap();
    sig["sig"] = json!(B64.encode(
        key.sign(format!("terrapkg-sig-v1:{}", hash(&bytes)).as_bytes())
            .to_bytes()
    ));
    assert!(verify(
        &bytes,
        &serde_json::to_vec(&sig).unwrap(),
        &c,
        &HighWater::default(),
        NOW
    )
    .is_err());
    let original: Value = serde_json::from_slice(&raw_signature(&bytes, &s)).unwrap();
    for (field, val) in [
        ("format", json!(2)),
        ("key", json!(c.keys[0].trim_end_matches('='))),
        ("sig", json!("?")),
        ("extra", json!(true)),
    ] {
        let mut bad = original.clone();
        bad[field] = val;
        assert!(verify(
            &bytes,
            &serde_json::to_vec(&bad).unwrap(),
            &c,
            &HighWater::default(),
            NOW
        )
        .is_err());
    }
    assert!(verify(
        &bytes,
        &vec![b' '; MAX_SIGNATURE_BYTES + 1],
        &c,
        &HighWater::default(),
        NOW
    )
    .is_err());
    assert!(strict_json::<Value>(br#"{"x":{"a":1,"\u0061":2}}"#, 100, 4).is_err());
    assert!(strict_json::<Value>(b"[[[[[]]]]]", 100, 4).is_err());
}
#[test]
fn expired_index_fails() {
    let (c, s, mut v) = fixture();
    assert_eq!(
        checked(&v, &s, &c, &HighWater::default(), NOW + DAY).unwrap_err(),
        EXPIRED
    );
    v["expires_ms"] = json!(NOW);
    assert!(checked(&v, &s, &c, &HighWater::default(), NOW).is_err());
}
#[test]
fn older_sequence_fails() {
    let (mut c, s, v) = fixture();
    let bytes = serde_json::to_vec(&v).unwrap();
    let mut last = HighWater {
        sequence: 3,
        hash: Some(hash(&bytes)),
        last_verified_ms: NOW,
    };
    assert!(checked(&v, &s, &c, &last, NOW).is_err());
    last.sequence = 2;
    assert!(checked(&v, &s, &c, &last, NOW).is_ok());
    let mut changed = v.clone();
    changed["plugins"][0]["name"] = json!("Changed");
    assert!(checked(&changed, &s, &c, &last, NOW).is_err());
    c.minimum_sequence = 3;
    assert!(checked(&v, &s, &c, &HighWater::default(), NOW).is_err());
}
#[test]
fn clock_and_lifetime_bounds() {
    let (c, s, v) = fixture();
    let last = HighWater {
        last_verified_ms: NOW + CLOCK_SKEW + 1,
        ..HighWater::default()
    };
    assert_eq!(checked(&v, &s, &c, &last, NOW).unwrap_err(), CLOCK_ERROR);
    for (generated, expires, ok) in [
        (NOW + CLOCK_SKEW, NOW + DAY, true),
        (NOW + CLOCK_SKEW + 1, NOW + DAY, false),
        (NOW, NOW + 7 * DAY, true),
        (NOW, NOW + 7 * DAY + 1, false),
        (NOW, NOW - 1, false),
    ] {
        let mut value = v.clone();
        value["generated_ms"] = json!(generated);
        value["expires_ms"] = json!(expires);
        assert_eq!(
            checked(&value, &s, &c, &HighWater::default(), NOW).is_ok(),
            ok
        );
    }
}
#[test]
fn oversized_index_fails() {
    let (c, s, _) = fixture();
    let bytes = vec![b' '; MAX_INDEX_BYTES + 1];
    assert!(verify(
        &bytes,
        &raw_signature(&bytes, &s),
        &c,
        &HighWater::default(),
        NOW
    )
    .is_err());
    assert!(strict_json::<Value>(b"{\"x\":1}", 7, 16).is_ok());
    assert!(strict_json::<Value>(b"{\"x\":1} ", 7, 16).is_err());
    assert!(
        strict_json::<Value>("[[[[[[[[[[[[[[[[[]]]]]]]]]]]]]]]]]".as_bytes(), 100, 16).is_err()
    );
}
#[test]
fn malformed_metadata_and_every_object_unknown_field_are_rejected() {
    let (c, s, v) = fixture();
    for pointer in ["", "/plugins/0", "/plugins/0/versions/0"] {
        let mut bad = v.clone();
        bad.pointer_mut(pointer).unwrap()["unknown"] = json!(1);
        assert!(checked(&bad, &s, &c, &HighWater::default(), NOW).is_err());
    }
    let cases = [
        ("/registry", json!("other")),
        ("/sequence", json!(0)),
        ("/sequence", json!(MAX_INTEGER + 1)),
        ("/sequence", json!(1.0)),
        ("/sequence", json!(-1)),
        ("/format", Value::Null),
        ("/plugins/0/id", json!("../notes")),
        ("/plugins/0/id", json!("a".repeat(64))),
        ("/plugins/0/name", json!("")),
        ("/plugins/0/name", json!("x".repeat(101))),
        ("/plugins/0/description", json!("x".repeat(2001))),
        ("/plugins/0/description", json!("\n")),
        ("/plugins/0/icon", json!("https://evil")),
        ("/plugins/0/icon", json!("a".repeat(65))),
        ("/plugins/0/publisher", json!("a")),
        ("/plugins/0/versions", json!([])),
        ("/plugins/0/versions/0/size", json!(0)),
        ("/plugins/0/versions/0/size", json!(MAX_PACKAGE_BYTES + 1)),
        ("/plugins/0/versions/0/digest", json!("A".repeat(64))),
        ("/plugins/0/versions/0/capabilities", json!(["a", "a"])),
        (
            "/plugins/0/versions/0/capabilities",
            json!(["x".repeat(257)]),
        ),
        ("/plugins/0/versions/0/capabilities", json!(["\u{0001}"])),
        ("/plugins/0/versions/0/api", json!("x".repeat(129))),
        ("/plugins/0/versions/0/min_host", json!("0.1.0-01")),
        ("/plugins/0/versions/0/version", json!("0.1.0-alpha")),
        ("/plugins/0/versions/0/yanked", json!("false")),
    ];
    for (path, value) in cases {
        let mut bad = v.clone();
        *bad.pointer_mut(path).unwrap() = value;
        assert!(
            checked(&bad, &s, &c, &HighWater::default(), NOW).is_err(),
            "{path}"
        );
    }
    let mut bad = v.clone();
    bad["plugins"] = json!(vec![v["plugins"][0].clone(); 1001]);
    assert!(checked(&bad, &s, &c, &HighWater::default(), NOW).is_err());
    bad = v.clone();
    bad["plugins"][0]["versions"] = json!(vec![v["plugins"][0]["versions"][0].clone(); 101]);
    assert!(checked(&bad, &s, &c, &HighWater::default(), NOW).is_err());
    bad = v.clone();
    bad["plugins"][0]["versions"][0]["capabilities"] =
        json!((0..129).map(|i| i.to_string()).collect::<Vec<_>>());
    assert!(checked(&bad, &s, &c, &HighWater::default(), NOW).is_err());
}
#[test]
fn unsafe_urls_and_origin_spellings_are_refused() {
    let (c, s, v) = fixture();
    for url in [
        "http://plugins.example.org/x",
        "https://evil.example.org/x",
        "https://plugins.example.org:443/x",
        "https://user@plugins.example.org/x",
        "https://plugins.example.org/v1/packages/../x",
        "https://plugins.example.org/%2fnotes",
        "https://plugins.example.org/x?token=1",
        "https://plugins.example.org/x#f",
        "https://plugins.example.org\\x",
    ] {
        let mut bad = v.clone();
        bad["plugins"][0]["versions"][0]["url"] = json!(url);
        assert!(checked(&bad, &s, &c, &HighWater::default(), NOW).is_err());
    }
    for origin in [
        "http://example.org",
        "https://localhost",
        "https://127.0.0.1",
        "https://[::1]",
        "https://plugins.terrarium.example",
        "https://a.local",
        "https://user@example.org",
        "https://example.org:443",
        "https://example.org/",
        "https://EXAMPLE.org",
    ] {
        let mut config = c.clone();
        config.origin = origin.into();
        assert!(config.validate().is_err(), "{origin}");
    }
}
#[test]
fn incompatible_versions_are_hidden() {
    let (c, s, v) = fixture();
    let i = checked(&v, &s, &c, &HighWater::default(), NOW).unwrap();
    let mut release = i.plugins[0].versions[0].clone();
    assert!(compatible(&release, HOST));
    release.min_host = "0.1.0".into();
    assert!(!compatible(&release, HOST));
    assert!(compatible(&release, "0.1.0+build"));
    release.api = "other".into();
    assert!(!compatible(&release, "1.0.0"));
    for host in ["bogus", "01.0.0", "0.1.0-01", "4294967296.0.0"] {
        assert!(version(host).is_none());
    }
    for (a, b) in [
        ("0.1.0-alpha.1", "0.1.0-alpha.2"),
        ("0.1.0-alpha.2", "0.1.0-alpha.10"),
        ("0.1.0-1", "0.1.0-a"),
        ("0.1.0-a", "0.1.0-a.1"),
        ("0.1.0-a", "0.1.0"),
        ("0.9.0", "0.10.0"),
    ] {
        assert!(version(a)
            .unwrap()
            .cmp_precedence(&version(b).unwrap())
            .is_lt());
    }
}
#[test]
fn updates_list_newer_compatible_versions_only() {
    let (c, s, v) = fixture();
    let mut i = checked(&v, &s, &c, &HighWater::default(), NOW).unwrap();
    let mut newer = i.plugins[0].versions[0].clone();
    newer.version = "0.10.0".into();
    i.plugins[0].versions.push(newer.clone());
    newer.version = "0.11.0".into();
    newer.min_host = "9.0.0".into();
    i.plugins[0].versions.push(newer);
    let mut row = Inventory {
        id: "notes".into(),
        version: "0.9.0".into(),
        removed: false,
        shadowed_by_dev: false,
    };
    assert_eq!(
        updates(&i, &[row.clone()], HOST),
        vec![("notes".into(), "0.10.0".into())]
    );
    for old in ["0.10.0", "0.11.0", "0.9.0-alpha", "not-a-version"] {
        row.version = old.into();
        assert!(updates(&i, &[row.clone()], HOST).is_empty());
    }
    row.version = "0.1.0".into();
    row.removed = true;
    assert!(updates(&i, &[row.clone()], HOST).is_empty());
    row.removed = false;
    row.shadowed_by_dev = true;
    assert!(updates(&i, &[row], HOST).is_empty());
}
#[test]
fn yanked_versions_are_not_offered() {
    let (c, s, v) = fixture();
    let mut i = checked(&v, &s, &c, &HighWater::default(), NOW).unwrap();
    let p = i.plugins[0].clone();
    let release = p.versions[0].clone();
    assert!(selection_unchanged(&i, REGISTRY_ID, 2, &p, &release, HOST).is_ok());
    i.plugins[0].versions[0].yanked = true;
    assert!(newest(&i.plugins[0], HOST).is_none());
    assert!(selection_unchanged(&i, REGISTRY_ID, 2, &p, &release, HOST).is_err());
    i.plugins[0] = p.clone();
    i.plugins[0].publisher = crate::signing_public_key(&[9; 32]);
    assert!(selection_unchanged(&i, REGISTRY_ID, 2, &p, &release, HOST).is_err());
    i.plugins.clear();
    assert!(selection_unchanged(&i, REGISTRY_ID, 2, &p, &release, HOST).is_err());
}
#[test]
fn digest_or_publisher_mismatch_is_refused() {
    let (c, s, v) = fixture();
    let i = checked(&v, &s, &c, &HighWater::default(), NOW).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("dist")).unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        include_bytes!("../tests/fixtures/limits/manifest.json"),
    )
    .unwrap();
    std::fs::write(dir.path().join("dist/plugin.wasm"), b"fixture").unwrap();
    let mut package = Package::from_dir(dir.path()).unwrap();
    package.sign(&s);
    let bytes = package.to_zip().unwrap();
    let mut p = i.plugins[0].clone();
    p.id = package.manifest().id.clone();
    let mut release = p.versions[0].clone();
    release.size = bytes.len() as u64;
    release.digest = package.digest();
    release.capabilities = capabilities(package.manifest())
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_ne!(hash(&bytes), release.digest);
    assert!(check_package(&bytes, &p, &release).is_ok());
    let mut bad = p.clone();
    bad.publisher = crate::signing_public_key(&[3; 32]);
    assert!(check_package(&bytes, &bad, &release).is_err());
    for field in ["digest", "size", "version", "api", "caps", "id"] {
        let mut v = release.clone();
        let mut plugin = p.clone();
        match field {
            "digest" => v.digest = "b".repeat(64),
            "size" => v.size += 1,
            "version" => v.version = "9.0.0".into(),
            "api" => v.api = "other".into(),
            "caps" => v.capabilities.push("hidden-permission".into()),
            _ => plugin.id = "other".into(),
        };
        assert!(check_package(&bytes, &plugin, &v).is_err(), "{field}");
    }
    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    assert!(check_package(&corrupt, &p, &release).is_err());
    let unsigned = Package::from_dir(dir.path()).unwrap().to_zip().unwrap();
    release.size = unsigned.len() as u64;
    assert!(check_package(&unsigned, &p, &release).is_err());
}

#[test]
fn inclusive_metadata_bounds_and_duplicate_arrays() {
    let (c, s, v) = fixture();
    let mut boundary = v.clone();
    boundary["plugins"][0]["id"] = json!("a".repeat(63));
    boundary["plugins"][0]["name"] = json!("n".repeat(100));
    boundary["plugins"][0]["description"] = json!("d".repeat(2000));
    boundary["plugins"][0]["icon"] = json!("i".repeat(64));
    boundary["plugins"][0]["versions"][0]["size"] = json!(MAX_PACKAGE_BYTES);
    boundary["plugins"][0]["versions"][0]["api"] = json!("a".repeat(128));
    boundary["plugins"][0]["versions"][0]["url"] = json!(format!(
        "{}/v1/packages/{}/0.1.0/{}.terrapkg",
        c.origin,
        "a".repeat(63),
        "a".repeat(64)
    ));
    boundary["plugins"][0]["versions"][0]["capabilities"] = json!((0..128)
        .map(|i| format!("{:03}{}", i, "x".repeat(253)))
        .collect::<Vec<_>>());
    assert!(checked(&boundary, &s, &c, &HighWater::default(), NOW).is_ok());
    let mut many = v.clone();
    many["plugins"][0]["versions"] = json!((0..100)
        .map(|i| {
            let mut release = v["plugins"][0]["versions"][0].clone();
            release["version"] = json!(format!("0.1.{i}"));
            release["url"] = json!(format!(
                "{}/v1/packages/notes/0.1.{i}/{}.terrapkg",
                c.origin,
                "a".repeat(64)
            ));
            release
        })
        .collect::<Vec<_>>());
    assert!(checked(&many, &s, &c, &HighWater::default(), NOW).is_ok());
    many = v.clone();
    many["plugins"] = json!((0..1000)
        .map(|i| {
            let mut plugin = v["plugins"][0].clone();
            plugin["id"] = json!(format!("notes-{i}"));
            plugin["versions"][0]["url"] = json!(format!(
                "{}/v1/packages/notes-{i}/0.1.0/{}.terrapkg",
                c.origin,
                "a".repeat(64)
            ));
            plugin
        })
        .collect::<Vec<_>>());
    assert!(checked(&many, &s, &c, &HighWater::default(), NOW).is_ok());
    many = v.clone();
    many["plugins"] = json!([v["plugins"][0].clone(), v["plugins"][0].clone()]);
    assert!(checked(&many, &s, &c, &HighWater::default(), NOW).is_err());
    many = v.clone();
    many["plugins"][0]["versions"] = json!([
        v["plugins"][0]["versions"][0].clone(),
        v["plugins"][0]["versions"][0].clone()
    ]);
    assert!(checked(&many, &s, &c, &HighWater::default(), NOW).is_err());
}

#[test]
fn declarative_api_is_supported_by_registry_selection() {
    let (c, s, mut v) = fixture();
    v["plugins"][0]["versions"][0]["api"] = json!(crate::DECLARATIVE_API_VERSION);
    let index = checked(&v, &s, &c, &HighWater::default(), NOW).unwrap();
    assert!(newest(&index.plugins[0], HOST).is_some());
}
