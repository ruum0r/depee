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
        current_spec: "^1.0.0".into(),
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
        update_args(Pm::Npm, &package, "2.0.0", true, false),
        vec![
            String::from("install"),
            String::from("--save-dev"),
            String::from("@scope/lib@2.0.0")
        ]
    );
    assert_eq!(
        update_args(Pm::Pnpm, &package, "1.1.0", false, false),
        vec![String::from("update"), String::from("@scope/lib")]
    );
    // Yarn Classic has no `up`; Berry's bare `up` would jump to latest.
    assert_eq!(
        update_args(Pm::Yarn, &package, "1.1.0", false, false),
        vec![String::from("upgrade"), String::from("@scope/lib")]
    );
    assert_eq!(
        update_args(Pm::Yarn, &package, "1.1.0", false, true),
        vec![String::from("up"), String::from("@scope/lib@^1.0.0")]
    );
    assert_eq!(
        update_args(Pm::Yarn, &package, "2.0.0", true, true),
        vec![
            String::from("add"),
            String::from("-D"),
            String::from("@scope/lib@2.0.0")
        ]
    );
}

#[test]
fn detects_yarn_flavor() {
    let dir = std::env::temp_dir().join(format!("depee-yarn-flavor-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    assert!(detect_yarn_berry(Some("yarn@4.9.1"), &dir));
    assert!(!detect_yarn_berry(Some("yarn@1.22.22"), &dir));
    assert!(!detect_yarn_berry(None, &dir));
    fs::write(dir.join("yarn.lock"), "# yarn lockfile v1\n").unwrap();
    assert!(!detect_yarn_berry(None, &dir));
    fs::write(dir.join("yarn.lock"), "__metadata:\n  version: 8\n").unwrap();
    assert!(detect_yarn_berry(None, &dir));
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(yarn_major("4.9.1\n"), Some(4));
    assert_eq!(yarn_major("N/A"), None);
}

#[test]
fn version_table_groups_patch_minor_and_major() {
    let mut package = Package {
        name: "@types/node".into(),
        current: "24.13.2".into(),
        current_spec: "^24.13.2".into(),
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
            dependency_specs: DependencySpecs::default(),
            engines_node: None,
            deprecated: None,
            license: None,
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
fn release_version_requires_v_prefixed_semver_tag() {
    assert_eq!(release_version("v1.2.3").as_deref(), Some("1.2.3"));
    assert_eq!(
        release_version("v1.2.3-rc.1").as_deref(),
        Some("1.2.3-rc.1")
    );
    assert_eq!(release_version("1.2.3"), None);
    assert_eq!(release_version("vv1.2.3"), None);
    assert_eq!(release_version("v1.2"), None);
}

#[test]
fn install_script_url_is_pinned_to_tag() {
    assert_eq!(
        install_script_url("1.2.3"),
        "https://raw.githubusercontent.com/ruum0r/depee/v1.2.3/install.sh"
    );
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
    let mut app = test_app();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(key(&mut app, ctrl_c).unwrap());
    app.query = Some(String::new());
    assert!(key(&mut app, ctrl_c).unwrap());
}

#[test]
fn requirement_matches_uses_npm_range_semantics() {
    // A bare version is an exact match in npm, not Cargo's caret.
    assert_eq!(requirement_matches("18.2.0", "18.2.0"), Some(true));
    assert_eq!(requirement_matches("18.2.0", "18.3.1"), Some(false));
    assert_eq!(requirement_matches("v18.2.0", "18.2.0"), Some(true));
    // Space-separated comparators are ANDed.
    assert_eq!(
        requirement_matches(">=14.17.0 <15.0.0", "14.18.0"),
        Some(true)
    );
    assert_eq!(
        requirement_matches(">=14.17.0 <15.0.0", "15.0.0"),
        Some(false)
    );
    assert_eq!(requirement_matches(">= 18.0.0", "v22.1.0"), Some(true));
    // Hyphen ranges are inclusive; a partial upper bound covers its minor.
    assert_eq!(requirement_matches("1.2.3 - 2.3.4", "2.0.0"), Some(true));
    assert_eq!(requirement_matches("1.2.3 - 2.3.4", "2.3.5"), Some(false));
    assert_eq!(requirement_matches("1.2.3 - 2.3", "2.3.9"), Some(true));
    // Partial bare versions and wildcards.
    assert_eq!(requirement_matches("1.2", "1.2.9"), Some(true));
    assert_eq!(requirement_matches("1.2", "1.3.0"), Some(false));
    assert_eq!(requirement_matches("16.x", "16.4.0"), Some(true));
    assert_eq!(requirement_matches("*", "1.0.0"), Some(true));
    assert_eq!(requirement_matches("", "1.0.0"), Some(true));
    // Alternatives.
    assert_eq!(
        requirement_matches("^18.18.0 || ^20.9.0 || >=21.1.0", "20.10.0"),
        Some(true)
    );
    assert_eq!(
        requirement_matches("^18.18.0 || ^20.9.0 || >=21.1.0", "19.0.0"),
        Some(false)
    );
    assert_eq!(requirement_matches("~> 1.2", "1.2.5"), Some(true));
}

#[test]
fn requirement_matches_reports_unknown_instead_of_guessing() {
    assert_eq!(requirement_matches("workspace:*", "1.0.0"), None);
    assert_eq!(requirement_matches("latest", "1.0.0"), None);
    assert_eq!(requirement_matches(">=18", "checking…"), None);
    assert_eq!(requirement_matches(">=18", "N/A"), None);
    assert_eq!(requirement_matches("1.0.0 -", "1.0.0"), None);
    // One unparseable alternative makes a non-match undecidable…
    assert_eq!(requirement_matches("^1.0.0 || npm:foo@2", "2.0.0"), None);
    // …but a parseable alternative that matches is still a match.
    assert_eq!(
        requirement_matches("^1.0.0 || npm:foo@2", "1.5.0"),
        Some(true)
    );
}

fn test_app() -> App {
    let (tx, _rx) = mpsc::unbounded_channel();
    App {
        cwd: PathBuf::from("."),
        project: "test".into(),
        pm: Pm::Npm,
        pm_version: String::new(),
        yarn_berry: false,
        node_version: String::new(),
        total: 0,
        packages: vec![],
        selected: 0,
        query: None,
        sort: Sort::Name,
        descending: false,
        loading: false,
        updating: false,
        confirm: false,
        confirm_scroll: 0,
        confirm_scroll_max: std::cell::Cell::new(0),
        version_picker: false,
        selected_target: None,
        target_latest: false,
        upgrade: false,
        status: String::new(),
        status_after_refresh: None,
        update_prompt: None,
        install_update: None,
        error: None,
        metadata_loading: HashMap::new(),
        metadata_requested: HashSet::new(),
        metadata_errors: HashMap::new(),
        installed_versions: RefCell::default(),
        metadata_limit: Arc::new(tokio::sync::Semaphore::new(1)),
        tx,
    }
}

fn test_package() -> Package {
    Package {
        name: "lib".into(),
        current: "1.0.0".into(),
        current_spec: "^1.0.0".into(),
        wanted: Some("1.1.0".into()),
        latest: Some("2.0.0".into()),
        kind: Kind::Prod,
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
    }
}

fn test_release(version: &str) -> Release {
    Release {
        version: version.into(),
        published: None,
        change: Change::Unknown,
        prerelease: false,
        dependencies: vec![],
        dependency_specs: DependencySpecs::default(),
        engines_node: None,
        deprecated: None,
        license: None,
    }
}

fn line_texts(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

#[test]
fn inspection_distinguishes_loading_failure_and_missing_metadata() {
    let mut app = test_app();
    app.packages = vec![test_package()];

    app.metadata_loading.insert(0, true);
    let text = line_texts(&inspection_lines(&app, &app.packages[0])).join("\n");
    assert!(text.contains("Loading npm Registry metadata"));
    assert!(!text.contains("unavailable"));

    app.metadata_loading.insert(0, false);
    app.metadata_errors.insert(0, "timed out".into());
    let text = line_texts(&inspection_lines(&app, &app.packages[0])).join("\n");
    assert!(text.contains("Failed to load npm Registry metadata · timed out"));
    assert!(!text.contains("unavailable"));
    assert!(!text.contains('✓'));

    app.metadata_errors.clear();
    let text = line_texts(&inspection_lines(&app, &app.packages[0])).join("\n");
    assert!(text.contains("Target version metadata unavailable"));
}

#[test]
fn installed_versions_are_cached_until_cleared() {
    let dir = std::env::temp_dir().join(format!("depee-installed-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let package_dir = dir.join("node_modules").join("@scope").join("peer");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(package_dir.join("package.json"), r#"{"version":"1.0.0"}"#).unwrap();
    let mut app = test_app();
    app.cwd = dir.join("nested");
    // Resolution walks up to the parent's node_modules, like Node does.
    assert_eq!(
        app.installed_version("@scope/peer").as_deref(),
        Some("1.0.0")
    );
    assert_eq!(app.installed_version("../escape"), None);
    fs::write(package_dir.join("package.json"), r#"{"version":"2.0.0"}"#).unwrap();
    assert_eq!(
        app.installed_version("@scope/peer").as_deref(),
        Some("1.0.0")
    );
    app.installed_versions.borrow_mut().clear();
    assert_eq!(
        app.installed_version("@scope/peer").as_deref(),
        Some("2.0.0")
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn confirm_mode_overrides_do_not_stick() {
    let press = |app: &mut App, code: KeyCode| {
        key(app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
    };
    let mut app = test_app();
    let mut package = test_package();
    package.releases = vec![test_release("2.0.0"), test_release("1.5.0")];
    package.release_index = 1;
    app.packages = vec![package];
    // Pretend metadata was already requested so `v` does not spawn a fetch.
    app.metadata_requested.insert(0);

    // Latest forces Upgrade and locks `m`; turning it off restores Compatible.
    press(&mut app, KeyCode::Char('u'));
    assert!(!app.upgrading());
    assert_eq!(app.effective_target().as_deref(), Some("1.1.0"));
    press(&mut app, KeyCode::Char('l'));
    assert!(app.upgrading());
    assert_eq!(app.effective_target().as_deref(), Some("2.0.0"));
    press(&mut app, KeyCode::Char('m'));
    assert!(!app.upgrade);
    press(&mut app, KeyCode::Char('l'));
    assert!(!app.upgrading());
    assert_eq!(app.effective_target().as_deref(), Some("1.1.0"));

    // Scrolling is bounded by the last draw and reset on reopen.
    app.confirm_scroll_max.set(2);
    for _ in 0..5 {
        press(&mut app, KeyCode::Down);
    }
    assert_eq!(app.confirm_scroll, 2);
    press(&mut app, KeyCode::Esc);
    assert!(!app.confirm);
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.confirm_scroll, 0);
    press(&mut app, KeyCode::Esc);

    // A picked version forces Upgrade only until the confirmation is dismissed.
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Enter);
    assert!(app.confirm);
    assert_eq!(app.selected_target.as_deref(), Some("1.5.0"));
    assert!(app.upgrading());
    press(&mut app, KeyCode::Char('l'));
    press(&mut app, KeyCode::Char('m'));
    assert!(!app.target_latest);
    assert!(!app.upgrade);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.selected_target, None);
    press(&mut app, KeyCode::Char('u'));
    assert!(!app.upgrading());
}

#[test]
fn dependency_diff_reports_changes_per_kind() {
    let specs = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(name, spec)| (name.to_string(), spec.to_string()))
            .collect::<HashMap<_, _>>()
    };
    let mut current = test_release("2.0.0");
    current.dependency_specs = DependencySpecs {
        dependencies: specs(&[("legacy-parser", "^1.5.0"), ("same", "^1.0.0")]),
        peer: specs(&[("react", "^18.0.0")]),
        optional: HashMap::new(),
    };
    let mut target = test_release("3.0.0");
    target.dependency_specs = DependencySpecs {
        dependencies: specs(&[("new-parser", "^2.0.0"), ("same", "^1.0.0")]),
        peer: specs(&[("react", "^19.0.0")]),
        // Same name under another kind is a separate change, not a move.
        optional: specs(&[("legacy-parser", "^1.5.0")]),
    };
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        dependency_diff(&current, &target),
        vec![
            (
                "dependencies".into(),
                "legacy-parser".into(),
                some("^1.5.0"),
                None
            ),
            (
                "dependencies".into(),
                "new-parser".into(),
                None,
                some("^2.0.0")
            ),
            (
                "peerDependencies".into(),
                "react".into(),
                some("^18.0.0"),
                some("^19.0.0")
            ),
            (
                "optionalDependencies".into(),
                "legacy-parser".into(),
                None,
                some("^1.5.0")
            ),
        ]
    );
    assert!(dependency_diff(&current, &current).is_empty());
}

#[test]
fn inspection_marks_incompatible_and_unknown_states() {
    let mut app = test_app();
    app.node_version = "v18.20.0".into();
    let mut package = test_package();
    let mut current = test_release("1.0.0");
    current.license = Some("MIT".into());
    let mut target = test_release("1.1.0");
    target.engines_node = Some(">=20".into());
    target.deprecated = Some("use lib2".into());
    target.dependency_specs.peer = HashMap::from([("not-installed-peer".into(), "^1.0.0".into())]);
    package.releases = vec![target, current];
    app.packages = vec![package];
    app.cwd = std::env::temp_dir().join("depee-no-node-modules");
    let text = line_texts(&inspection_lines(&app, &app.packages[0])).join("\n");
    assert!(text.contains("✗ Requires Node.js >=20 (current v18.20.0)"));
    assert!(text.contains("? unresolved"));
    assert!(text.contains("! Deprecated · use lib2"));
    assert!(text.contains("? MIT → unknown (target unknown)"));
}

#[test]
fn confirm_scroll_accepts_every_nav_key_and_ignores_ctrl_letters() {
    let mut app = test_app();
    app.packages = vec![test_package()];
    app.open_confirm();
    app.confirm_scroll_max.set(10);
    let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
    for k in [plain(KeyCode::Down), plain(KeyCode::Char('j')), ctrl('n')] {
        key(&mut app, k).unwrap();
    }
    assert_eq!(app.confirm_scroll, 3);
    key(&mut app, ctrl('p')).unwrap();
    assert_eq!(app.confirm_scroll, 2);
    // Ctrl+n scrolls instead of cancelling; Ctrl+y/l/m do not act as y/l/m.
    for c in ['y', 'l', 'm'] {
        key(&mut app, ctrl(c)).unwrap();
    }
    assert!(app.confirm);
    assert!(!app.updating);
    assert!(!app.target_latest);
    assert!(!app.upgrade);
    key(&mut app, plain(KeyCode::Char('k'))).unwrap();
    key(&mut app, plain(KeyCode::Up)).unwrap();
    assert_eq!(app.confirm_scroll, 0);
    key(&mut app, plain(KeyCode::Char('n'))).unwrap();
    assert!(!app.confirm);
}
