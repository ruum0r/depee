use super::*;

#[test]
fn classifies_versions() {
    assert_eq!(change("1.2.3", Some("1.2.4")), Change::Patch);
    assert_eq!(change("1.2.3", Some("1.3.0")), Change::Minor);
    assert_eq!(change("1.2.3", Some("2.0.0")), Change::Major);
    assert_eq!(change("workspace:*", Some("2.0.0")), Change::Unknown);
    assert_eq!(change("1.0.0", Some("2.0.0-beta.1")), Change::Pre);
}

#[test]
fn pm_override_wins() {
    assert_eq!(
        detect(Some(Pm::Bun), Some("npm@10"), Path::new("/does-not-exist")).unwrap(),
        Pm::Bun
    );
}

#[test]
fn npm_json_parses() {
    let v = serde_json::json!({"vite":{"current":"1.0.0","wanted":"1.2.0","latest":"2.0.0"}});
    let p = parse_outdated(Pm::Npm, &v).unwrap();
    assert_eq!(p[0].0, "vite");
    assert_eq!(p[0].2.as_deref(), Some("1.2.0"));
}

#[test]
fn pnpm_json_object_parses() {
    let v = serde_json::json!({
        "vite": {
            "current": "6.0.1",
            "wanted": "6.4.0",
            "latest": "7.1.2",
            "dependencyType": "devDependencies"
        }
    });
    let packages = parse_outdated(Pm::Pnpm, &v).unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].0, "vite");
    assert_eq!(packages[0].1, "6.0.1");
    assert_eq!(packages[0].2.as_deref(), Some("6.4.0"));
    assert_eq!(packages[0].3.as_deref(), Some("7.1.2"));
}

#[test]
fn update_commands_keep_dependency_kind() {
    let package = Package {
        name: "@scope/lib".into(),
        current: "1.0.0".into(),
        wanted: Some("1.1.0".into()),
        latest: Some("2.0.0".into()),
        kind: Kind::Dev,
        change: Change::Major,
        description: None,
        license: None,
        repo: None,
        homepage: None,
        current_published: None,
        published: None,
        downloads: None,
        releases: vec![],
        release_index: 0,
    };
    assert_eq!(
        update_args(Pm::Npm, &package, "2.0.0", true),
        vec![
            String::from("install"),
            String::from("--save-dev"),
            String::from("@scope/lib@2.0.0")
        ]
    );
    assert_eq!(
        update_args(Pm::Pnpm, &package, "1.1.0", false),
        vec![String::from("update"), String::from("@scope/lib")]
    );
}

#[test]
fn version_table_groups_patch_minor_and_major() {
    let mut package = Package {
        name: "@types/node".into(),
        current: "24.13.2".into(),
        wanted: Some("24.15.0".into()),
        latest: Some("26.6.5".into()),
        kind: Kind::Dev,
        change: Change::Major,
        description: None,
        license: None,
        repo: None,
        homepage: None,
        current_published: None,
        published: None,
        downloads: None,
        releases: vec![],
        release_index: 0,
    };
    for version in ["24.13.2", "24.13.5", "24.15.0", "25.0.0", "26.6.5"] {
        package.releases.push(Release {
            version: version.into(),
            published: None,
            change: Change::Unknown,
            prerelease: false,
            dependencies: vec![],
        });
    }
    assert_eq!(
        version_lanes(&package),
        ("24.13.5".into(), "24.15.0".into(), "26.6.5".into())
    );
}
