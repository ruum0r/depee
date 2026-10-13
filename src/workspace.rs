//! Workspace members: the `workspaces` globs of `package.json` and the
//! `packages:` list of `pnpm-workspace.yaml`, expanded below the root.
//!
//! Expansion only reads the root's subtree. It never follows symlinked
//! directories or manifests, skips `node_modules` and dot-directories, rejects
//! patterns that could leave the root (`..`, absolute paths), and stops after
//! `MAX_DEPTH` levels or `MAX_ENTRIES` directory entries.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

/// Directory levels below the root that `**` may reach.
const MAX_DEPTH: usize = 8;
/// Directory entries read before the walk gives up on the rest.
const MAX_ENTRIES: usize = 50_000;
/// Larger manifests and workspace files are ignored.
const MAX_FILE_BYTES: u64 = 1 << 20;

/// A project in the workspace, the root included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub dir: PathBuf,
    /// Relative to the root, `/`-separated; empty for the root itself.
    pub path: String,
    /// The manifest `name`, with control characters replaced.
    pub name: Option<String>,
}

impl Member {
    pub fn is_root(&self) -> bool {
        self.path.is_empty()
    }

    /// The relative path, or `(root)`.
    pub fn label(&self) -> &str {
        if self.is_root() {
            "(root)"
        } else {
            &self.path
        }
    }
}

/// The root and its members sorted by relative path, the root first. Only
/// directories with a regular `package.json` file count.
pub fn members(root: &Path) -> Vec<Member> {
    let (include, exclude) = patterns(root);
    let mut members: Vec<Member> = member_at(root, String::new()).into_iter().collect();
    let mut stack = vec![(root.to_path_buf(), Vec::<String>::new())];
    let mut budget = MAX_ENTRIES;
    while let Some((dir, rel)) = stack.pop() {
        if budget == 0 {
            break;
        }
        if rel.len() >= MAX_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            // `file_type` does not follow symlinks, so linked directories are skipped.
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name.starts_with('.') || name == "node_modules" || name.chars().any(char::is_control)
            {
                continue;
            }
            let mut child = rel.clone();
            child.push(name);
            if !include.iter().any(|glob| glob.may_contain(&child)) {
                continue;
            }
            let path = entry.path();
            if include.iter().any(|glob| glob.matches(&child))
                && !exclude.iter().any(|glob| glob.matches(&child))
            {
                members.extend(member_at(&path, child.join("/")));
            }
            stack.push((path, child));
        }
    }
    members.sort_by(|a, b| a.path.cmp(&b.path));
    members
}

/// Finds a member by relative path (`.` or `(root)` for the root), then by
/// package name.
pub fn find<'a>(members: &'a [Member], query: &str) -> Result<&'a Member, String> {
    let path = normalize(query);
    let by_path = members.iter().find(|member| {
        member.path == path || (member.is_root() && matches!(query, "." | "(root)"))
    });
    if let Some(member) = by_path {
        return Ok(member);
    }
    let named: Vec<_> = members
        .iter()
        .filter(|member| member.name.as_deref() == Some(query))
        .collect();
    match named.as_slice() {
        [member] => Ok(member),
        [] => Err(format!(
            "Unknown workspace member: {query} (members: {})",
            members
                .iter()
                .map(Member::label)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(format!(
            "Several workspace members are named {query}; use a path instead"
        )),
    }
}

fn member_at(dir: &Path, path: String) -> Option<Member> {
    let manifest = read_regular(&dir.join("package.json"))?;
    let name = serde_json::from_str::<Value>(&manifest)
        .ok()
        .and_then(|value| value.get("name")?.as_str().map(sanitize));
    Some(Member {
        dir: dir.to_path_buf(),
        path,
        name,
    })
}

/// Replaces control characters, which could drive the terminal when drawn.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Reads a small regular file; symlinks and anything else are ignored.
fn read_regular(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// Include and exclude globs declared by the root. Unusable patterns are dropped.
fn patterns(root: &Path) -> (Vec<Glob>, Vec<Glob>) {
    let mut raw = vec![];
    if let Some(manifest) = read_regular(&root.join("package.json")) {
        raw.extend(package_json_patterns(&manifest));
    }
    if let Some(yaml) = read_regular(&root.join("pnpm-workspace.yaml")) {
        raw.extend(pnpm_packages(&yaml));
    }
    let (mut include, mut exclude) = (vec![], vec![]);
    for pattern in raw {
        match pattern.strip_prefix('!') {
            Some(pattern) => exclude.extend(Glob::new(pattern)),
            None => include.extend(Glob::new(&pattern)),
        }
    }
    (include, exclude)
}

/// `workspaces` as an array, or as `{ "packages": [...] }`.
fn package_json_patterns(manifest: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(manifest) else {
        return vec![];
    };
    let list = match value.get("workspaces") {
        Some(Value::Array(list)) => list,
        Some(Value::Object(fields)) => match fields.get("packages") {
            Some(Value::Array(list)) => list,
            _ => return vec![],
        },
        _ => return vec![],
    };
    list.iter()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// Entries of the top-level `packages:` key of `pnpm-workspace.yaml`. Handles
/// block lists (`- item`, quoted or not) and one-line flow lists (`[a, b]`),
/// with comments; other keys such as `catalog:` are ignored.
pub fn pnpm_packages(yaml: &str) -> Vec<String> {
    let mut items = vec![];
    let mut in_packages = false;
    for line in yaml.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let top_level = !line.starts_with([' ', '\t']);
        if let Some(rest) = trimmed.strip_prefix('-').filter(|_| in_packages) {
            if rest.is_empty() || rest.starts_with([' ', '\t']) {
                items.extend(scalar(rest));
                continue;
            }
        }
        if !top_level {
            continue;
        }
        in_packages = false;
        if let Some(rest) = trimmed.strip_prefix("packages:") {
            let rest = strip_comment(rest).trim();
            if let Some(flow) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                items.extend(flow.split(',').filter_map(scalar));
            } else {
                in_packages = rest.is_empty();
            }
        }
    }
    items
}

/// A YAML scalar: quoted (up to the closing quote) or plain (up to ` #`).
fn scalar(text: &str) -> Option<String> {
    let text = text.trim();
    let value = match text.chars().next()? {
        quote @ ('\'' | '"') => {
            let inner = &text[1..];
            &inner[..inner.find(quote)?]
        }
        _ => strip_comment(text).trim(),
    };
    (!value.is_empty()).then(|| value.to_string())
}

fn strip_comment(text: &str) -> &str {
    if text.starts_with('#') {
        return "";
    }
    text.find(" #").map_or(text, |end| &text[..end])
}

/// Strips `./` prefixes and trailing slashes.
fn normalize(path: &str) -> String {
    let mut path = path.trim();
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path.trim_end_matches('/').to_string()
}

#[derive(Debug, PartialEq, Eq)]
enum Segment {
    /// `**`: any number of directories, including none.
    Any,
    /// A name where `*` matches any run of characters.
    Name(String),
}

/// A workspace glob, relative to the root.
#[derive(Debug, PartialEq, Eq)]
pub struct Glob(Vec<Segment>);

impl Glob {
    /// `None` for patterns that are empty, absolute, use `\`, or contain `..`.
    pub fn new(pattern: &str) -> Option<Self> {
        let pattern = normalize(pattern);
        if pattern.starts_with('/') || pattern.contains('\\') || pattern.contains(':') {
            return None;
        }
        let mut segments = vec![];
        for part in pattern.split('/') {
            match part {
                "" | "." => {}
                ".." => return None,
                "**" => segments.push(Segment::Any),
                name => segments.push(Segment::Name(name.to_string())),
            }
        }
        (!segments.is_empty()).then_some(Self(segments))
    }

    pub fn matches<S: AsRef<str>>(&self, path: &[S]) -> bool {
        matches(&self.0, path)
    }

    /// Whether `dir` or anything below it could match.
    pub fn may_contain<S: AsRef<str>>(&self, dir: &[S]) -> bool {
        may_contain(&self.0, dir)
    }
}

fn matches<S: AsRef<str>>(glob: &[Segment], path: &[S]) -> bool {
    match glob.split_first() {
        None => path.is_empty(),
        Some((Segment::Any, rest)) => (0..=path.len()).any(|skip| matches(rest, &path[skip..])),
        Some((Segment::Name(name), rest)) => path
            .split_first()
            .is_some_and(|(first, tail)| name_matches(name, first.as_ref()) && matches(rest, tail)),
    }
}

fn may_contain<S: AsRef<str>>(glob: &[Segment], dir: &[S]) -> bool {
    match (glob.split_first(), dir.split_first()) {
        (_, None) => true,
        (None, Some(_)) => false,
        (Some((Segment::Any, _)), Some(_)) => true,
        (Some((Segment::Name(name), rest)), Some((first, tail))) => {
            name_matches(name, first.as_ref()) && may_contain(rest, tail)
        }
    }
}

/// Matches one path segment; `*` stands for any run of characters.
fn name_matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<_> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob(pattern: &str) -> Glob {
        Glob::new(pattern).unwrap()
    }

    fn split(path: &str) -> Vec<&str> {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }

    #[test]
    fn pnpm_packages_reads_only_the_packages_list() {
        let yaml = "\
# workspace
packages:
  # apps first
  - apps/*
  - 'packages/*'   # quoted
  - \"tools/**\"
  -   '!**/test/**'
  - plain # trailing comment
catalog:
  - not-a-package
  react: ^18.0.0
packagesExtra:
  - nope
";
        assert_eq!(
            pnpm_packages(yaml),
            ["apps/*", "packages/*", "tools/**", "!**/test/**", "plain"]
        );
        // Unindented items and flow lists are valid YAML too.
        assert_eq!(pnpm_packages("packages:\n- a\n- b\n"), ["a", "b"]);
        assert_eq!(
            pnpm_packages("packages: ['a/*', \"b\", c] # x\n"),
            ["a/*", "b", "c"]
        );
        assert!(pnpm_packages("catalog:\n  - a\n").is_empty());
        assert!(pnpm_packages("").is_empty());
    }

    #[test]
    fn globs_match_segments_double_stars_and_reject_escapes() {
        assert!(glob("packages/*").matches(&split("packages/web")));
        assert!(!glob("packages/*").matches(&split("packages/web/src")));
        assert!(!glob("packages/*").matches(&split("packages")));
        assert!(glob("./packages/web/").matches(&split("packages/web")));
        assert!(glob("packages/ui-*").matches(&split("packages/ui-kit")));
        assert!(!glob("packages/ui-*").matches(&split("packages/web")));
        assert!(glob("*-app").matches(&split("my-app")));
        assert!(glob("a*b*c").matches(&split("abxbc")));
        assert!(!glob("a*b*c").matches(&split("acb")));
        assert!(glob("apps/**").matches(&split("apps/x/y")));
        assert!(glob("apps/**").matches(&split("apps")));
        assert!(glob("**/test/**").matches(&split("packages/test/fixture")));
        assert!(glob("**/test/**").matches(&split("test")));
        assert!(!glob("**/test/**").matches(&split("packages/testing")));

        assert!(glob("packages/*").may_contain(&split("packages")));
        assert!(glob("packages/*").may_contain(&split("packages/web")));
        assert!(!glob("packages/*").may_contain(&split("packages/web/src")));
        assert!(!glob("packages/*").may_contain(&split("apps")));
        assert!(glob("**/x").may_contain(&split("deep/er")));

        for bad in [
            "../outside",
            "packages/../../x",
            "/abs",
            "C:/x",
            "a\\b",
            "",
            ".",
            "./",
        ] {
            assert_eq!(Glob::new(bad), None, "{bad}");
        }
    }

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn paths(members: &[Member]) -> Vec<&str> {
        members.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn members_expand_globs_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = &dir.path().join("repo");
        write(
            &root.join("package.json"),
            r#"{"name":"mono","workspaces":["packages/*","../escape"]}"#,
        );
        write(
            &root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'apps/**'\n  - '!apps/legacy'\n",
        );
        write(&root.join("packages/web/package.json"), r#"{"name":"web"}"#);
        write(&root.join("packages/no-name/package.json"), "{}");
        write(&root.join("packages/empty/README.md"), "");
        write(&root.join("packages/.hidden/package.json"), "{}");
        write(&root.join("packages/node_modules/package.json"), "{}");
        write(
            &root.join("apps/site/package.json"),
            r#"{"name":"si\u001bte"}"#,
        );
        write(&root.join("apps/site/nested/package.json"), "{}");
        write(&root.join("apps/legacy/package.json"), "{}");
        write(&root.join("apps/x/node_modules/dep/package.json"), "{}");
        write(&root.join("other/package.json"), "{}");
        // `../escape` would reach this sibling of the root; it is rejected.
        write(&dir.path().join("escape/package.json"), "{}");

        let members = members(root);
        assert_eq!(
            paths(&members),
            [
                "",
                "apps/site",
                "apps/site/nested",
                "packages/no-name",
                "packages/web"
            ]
        );
        assert_eq!(members[0].name.as_deref(), Some("mono"));
        assert_eq!(members[0].label(), "(root)");
        assert_eq!(members[1].name.as_deref(), Some("si?te"));
        assert_eq!(members[3].name, None);
        assert_eq!(members[4].dir, root.join("packages/web"));

        assert_eq!(find(&members, "web").unwrap().path, "packages/web");
        assert_eq!(
            find(&members, "./packages/web/").unwrap().path,
            "packages/web"
        );
        assert!(find(&members, ".").unwrap().is_root());
        assert!(find(&members, "(root)").unwrap().is_root());
        let error = find(&members, "nope").unwrap_err();
        assert!(error.contains("(root), apps/site"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn members_do_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let outside = dir.path().join("outside");
        write(
            &root.join("package.json"),
            r#"{"workspaces":["packages/**"]}"#,
        );
        write(&outside.join("pkg/package.json"), r#"{"name":"outside"}"#);
        write(&root.join("packages/real/package.json"), "{}");
        std::os::unix::fs::symlink(&outside, root.join("packages/linked")).unwrap();
        // A loop back to the root must not be walked either.
        std::os::unix::fs::symlink(&root, root.join("packages/real/loop")).unwrap();
        // A symlinked manifest does not make a member.
        fs::create_dir_all(root.join("packages/fake")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("pkg/package.json"),
            root.join("packages/fake/package.json"),
        )
        .unwrap();

        assert_eq!(paths(&members(&root)), ["", "packages/real"]);
    }

    #[test]
    fn duplicate_names_must_be_chosen_by_path() {
        let member = |path: &str| Member {
            dir: PathBuf::from(path),
            path: path.into(),
            name: Some("same".into()),
        };
        let members = [member("a"), member("b")];
        assert!(find(&members, "same").unwrap_err().contains("use a path"));
        assert_eq!(find(&members, "b").unwrap().path, "b");
    }
}
