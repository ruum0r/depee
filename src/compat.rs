//! Compatibility checks between releases: npm range matching and dependency diffs.

use std::collections::BTreeSet;

use crate::package::Release;

/// Whether `version` satisfies the npm range `requirement`, or `None` when
/// that cannot be decided (unparseable range or version).
pub fn requirement_matches(requirement: &str, version: &str) -> Option<bool> {
    let version = semver::Version::parse(version.trim_start_matches('v')).ok()?;
    let mut parsed_any = false;
    let mut all_parsed = true;
    for alternative in requirement.split("||") {
        match npm_comparator_set(alternative).and_then(|set| semver::VersionReq::parse(&set).ok()) {
            Some(requirement) => {
                parsed_any = true;
                if requirement.matches(&version) {
                    return Some(true);
                }
            }
            None => all_parsed = false,
        }
    }
    (parsed_any && all_parsed).then_some(false)
}

// Translate one npm comparator set (the part between `||`) into Cargo's
// `VersionReq` syntax. npm separates comparators with spaces, supports hyphen
// ranges, and treats a bare version as an exact match, whereas Cargo uses
// commas and treats a bare version as `^`. Partial versions and wildcards
// already mean the same thing in both once an explicit operator is present.
fn npm_comparator_set(range: &str) -> Option<String> {
    const OPERATORS: [&str; 8] = [">=", "<=", "~>", ">", "<", "=", "~", "^"];
    let mut tokens: Vec<String> = vec![];
    for token in range.split_whitespace() {
        match tokens.last_mut() {
            // `>= 1.2.3` is valid npm syntax; glue the operator to its version.
            Some(last) if OPERATORS.contains(&last.as_str()) => last.push_str(token),
            _ => tokens.push(token.to_string()),
        }
    }
    let mut comparators = vec![];
    let mut index = 0;
    while index < tokens.len() {
        if tokens.get(index + 1).map(String::as_str) == Some("-") {
            let upper = tokens.get(index + 2)?;
            comparators.push(format!(">={}", npm_version(&tokens[index])));
            comparators.push(format!("<={}", npm_version(upper)));
            index += 3;
            continue;
        }
        let token = &tokens[index];
        let operator = OPERATORS
            .iter()
            .find(|operator| token.starts_with(*operator))
            .copied()
            .unwrap_or("");
        let version = npm_version(&token[operator.len()..]);
        if version.is_empty() {
            return None;
        }
        let operator = match operator {
            "~>" => "~",
            "" => "=",
            operator => operator,
        };
        comparators.push(if matches!(version, "*" | "x" | "X") {
            "*".to_string()
        } else {
            format!("{operator}{version}")
        });
        index += 1;
    }
    Some(if comparators.is_empty() {
        "*".to_string()
    } else {
        comparators.join(", ")
    })
}

fn npm_version(version: &str) -> &str {
    version.trim_start_matches('=').trim_start_matches('v')
}

/// A dependency whose declared range differs between two releases.
/// `old`/`new` is `None` when the dependency was added/removed.
#[derive(Debug, PartialEq, Eq)]
pub struct DependencyChange {
    pub group: &'static str,
    pub name: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

/// Changes grouped by dependency field, then sorted by name.
pub fn dependency_diff(current: &Release, target: &Release) -> Vec<DependencyChange> {
    let mut changes = Vec::new();
    for ((group, old_group), (_, new_group)) in current
        .dependency_specs
        .groups()
        .into_iter()
        .zip(target.dependency_specs.groups())
    {
        let names: BTreeSet<&String> = old_group.keys().chain(new_group.keys()).collect();
        for name in names {
            let old = old_group.get(name).cloned();
            let new = new_group.get(name).cloned();
            if old != new {
                changes.push(DependencyChange {
                    group,
                    name: name.clone(),
                    old,
                    new,
                });
            }
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::package::{fixtures::release, DependencySpecs};

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

    #[test]
    fn dependency_diff_reports_changes_per_kind() {
        let specs = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(name, spec)| (name.to_string(), spec.to_string()))
                .collect::<HashMap<_, _>>()
        };
        let mut current = release("2.0.0");
        current.dependency_specs = DependencySpecs {
            dependencies: specs(&[("legacy-parser", "^1.5.0"), ("same", "^1.0.0")]),
            peer: specs(&[("react", "^18.0.0")]),
            optional: HashMap::new(),
        };
        let mut target = release("3.0.0");
        target.dependency_specs = DependencySpecs {
            dependencies: specs(&[("new-parser", "^2.0.0"), ("same", "^1.0.0")]),
            peer: specs(&[("react", "^19.0.0")]),
            // Same name under another kind is a separate change, not a move.
            optional: specs(&[("legacy-parser", "^1.5.0")]),
        };
        let change = |group, name: &str, old: Option<&str>, new: Option<&str>| DependencyChange {
            group,
            name: name.into(),
            old: old.map(Into::into),
            new: new.map(Into::into),
        };
        assert_eq!(
            dependency_diff(&current, &target),
            [
                change("dependencies", "legacy-parser", Some("^1.5.0"), None),
                change("dependencies", "new-parser", None, Some("^2.0.0")),
                change(
                    "peerDependencies",
                    "react",
                    Some("^18.0.0"),
                    Some("^19.0.0")
                ),
                change(
                    "optionalDependencies",
                    "legacy-parser",
                    None,
                    Some("^1.5.0")
                ),
            ]
        );
        assert!(dependency_diff(&current, &current).is_empty());
    }
}
