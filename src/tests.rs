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

#[test]
fn pending_update_respects_version_and_skip() {
    let cache = |latest: &str, skipped: Option<&str>| UpdateCheckCache {
        checked_at: Utc::now(),
        latest_version: Some(latest.into()),
        skipped_version: skipped.map(Into::into),
    };
    assert_eq!(
        pending_update(&cache("99.0.0", None)).as_deref(),
        Some("99.0.0")
    );
    assert_eq!(pending_update(&cache("99.0.0", Some("99.0.0"))), None);
    assert_eq!(
        pending_update(&cache("99.1.0", Some("99.0.0"))).as_deref(),
        Some("99.1.0")
    );
    assert_eq!(pending_update(&cache("0.0.1", None)), None);
}

#[test]
fn update_cache_without_skipped_version_parses() {
    let cache: UpdateCheckCache =
        serde_json::from_str(r#"{"checked_at":"2026-01-01T00:00:00Z","latest_version":"9.9.9"}"#)
            .unwrap();
    assert_eq!(cache.skipped_version, None);
}

#[test]
fn nav_keys_are_shared() {
    let key = |code, modifiers| KeyEvent::new(code, modifiers);
    let none = KeyModifiers::NONE;
    let ctrl = KeyModifiers::CONTROL;
    assert_eq!(nav_delta(&key(KeyCode::Up, none), true), Some(-1));
    assert_eq!(nav_delta(&key(KeyCode::Char('p'), ctrl), true), Some(-1));
    assert_eq!(nav_delta(&key(KeyCode::Char('k'), none), true), Some(-1));
    assert_eq!(nav_delta(&key(KeyCode::Down, none), false), Some(1));
    assert_eq!(nav_delta(&key(KeyCode::Char('n'), ctrl), false), Some(1));
    assert_eq!(nav_delta(&key(KeyCode::Char('j'), none), false), None);
    assert_eq!(nav_delta(&key(KeyCode::Char('n'), none), true), None);
    assert_eq!(step(0, -1, 3), 0);
    assert_eq!(step(2, 1, 3), 2);
    assert_eq!(step(0, 1, 0), 0);
}

#[test]
fn ctrl_c_quits_instead_of_opening_changelog() {
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut app = App {
        cwd: PathBuf::from("."),
        project: "test".into(),
        pm: Pm::Npm,
        pm_version: String::new(),
        total: 0,
        packages: vec![],
        selected: 0,
        query: None,
        sort: Sort::Name,
        descending: false,
        loading: false,
        updating: false,
        confirm: false,
        version_picker: false,
        selected_target: None,
        target_latest: false,
        upgrade: false,
        status: String::new(),
        update_prompt: None,
        install_update: None,
        error: None,
        metadata_loading: HashMap::new(),
        metadata_requested: HashSet::new(),
        metadata_limit: Arc::new(tokio::sync::Semaphore::new(1)),
        tx,
    };
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(key(&mut app, ctrl_c).unwrap());
    app.query = Some(String::new());
    assert!(key(&mut app, ctrl_c).unwrap());
}
