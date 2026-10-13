//! Reading `yarn outdated --json` output, which is NDJSON on Yarn Classic.

use anyhow::{anyhow, Result};
use serde_json::Value;

/// Yarn Berry (2+) has no `outdated` command and prints this usage error to
/// stdout instead.
const BERRY_MISSING_OUTDATED: &str = "Couldn't find a script named \"outdated\"";

/// Longest excerpt of unexpected output quoted in an error.
const EXCERPT_CHARS: usize = 160;

/// Collects the JSON documents of Yarn Classic's line-per-document output.
/// Non-JSON lines are skipped, but output with no JSON at all from a failing
/// command is an error, so a usage error never looks like "nothing outdated".
pub(super) fn parse_ndjson(stdout: &str, failed: bool) -> Result<Value> {
    let plain = strip_ansi(stdout);
    if plain.contains(BERRY_MISSING_OUTDATED) {
        return Err(anyhow!(
            "Yarn Berry (2+) has no `outdated` command; depee cannot list outdated packages for Yarn 2+ yet"
        ));
    }
    let documents: Vec<Value> = plain
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if documents.is_empty() && failed {
        let first = plain.lines().map(str::trim).find(|line| !line.is_empty());
        if let Some(first) = first {
            let mut excerpt: String = first.chars().take(EXCERPT_CHARS).collect();
            if first.chars().count() > EXCERPT_CHARS {
                excerpt.push('…');
            }
            return Err(anyhow!("yarn outdated printed no JSON: {excerpt}"));
        }
    }
    Ok(Value::Array(documents))
}

/// Removes terminal escape sequences (CSI such as colors, and two-byte ESC
/// sequences) so messages render as plain text.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters and intermediates, then one final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs until BEL or ESC \.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.next().is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from Yarn 4.10.3 (stdout, exit code 1).
    const BERRY: &str = "\u{1b}[31m\u{1b}[1mUsage Error\u{1b}[22m\u{1b}[39m: Couldn't find a script named \"outdated\".\n\n\u{1b}[1m$ \u{1b}[22myarn run [--inspect] [--inspect-brk] [-T,--top-level] [-B,--binaries-only] [--require #0] <scriptName> ...\n";

    const CLASSIC: &str = concat!(
        "{\"type\":\"warning\",\"data\":\"package.json: No license field\"}\n",
        "{\"type\":\"info\",\"data\":\"Color legend : ...\"}\n",
        "{\"type\":\"table\",\"data\":{\"head\":[\"Package\",\"Current\",\"Wanted\",\"Latest\",\"Package Type\",\"URL\"],",
        "\"body\":[[\"left-pad\",\"1.0.0\",\"1.3.0\",\"2.0.0\",\"dependencies\",\"\"]]}}\n",
    );

    #[test]
    fn classic_ndjson_keeps_every_document() {
        let documents = parse_ndjson(CLASSIC, true).unwrap();
        let documents = documents.as_array().unwrap();
        assert_eq!(documents.len(), 3);
        assert_eq!(documents[2]["type"], "table");
        assert_eq!(documents[2]["data"]["body"][0][0], "left-pad");
    }

    #[test]
    fn classic_skips_stray_text_lines() {
        let output = format!("yarn outdated v1.22.22\n{CLASSIC}Done in 0.5s.\n");
        let documents = parse_ndjson(&output, true).unwrap();
        assert_eq!(documents.as_array().unwrap().len(), 3);
    }

    #[test]
    fn berry_usage_error_is_reported() {
        let error = parse_ndjson(BERRY, true).unwrap_err().to_string();
        assert!(
            error.contains("Yarn Berry (2+) has no `outdated` command"),
            "{error}"
        );
        assert!(!error.contains('\u{1b}'), "{error}");
        // Even if the exit code were not reported as a failure.
        assert!(parse_ndjson(BERRY, false).is_err());
    }

    #[test]
    fn failing_output_without_json_is_an_error() {
        let error = parse_ndjson("\u{1b}[31merror\u{1b}[39m Something broke\n", true)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "yarn outdated printed no JSON: error Something broke"
        );
        let long = "x".repeat(500);
        let error = parse_ndjson(&long, true).unwrap_err().to_string();
        assert!(error.ends_with('…'), "{error}");
        assert!(error.chars().count() < 220, "{error}");
    }

    #[test]
    fn empty_or_successful_output_is_empty() {
        assert_eq!(parse_ndjson("", true).unwrap(), Value::Array(vec![]));
        assert_eq!(parse_ndjson("\n  \n", true).unwrap(), Value::Array(vec![]));
        assert_eq!(
            parse_ndjson("Done in 0.1s.\n", false).unwrap(),
            Value::Array(vec![])
        );
    }

    #[test]
    fn strips_escape_sequences() {
        assert_eq!(strip_ansi("\u{1b}[1;31mred\u{1b}[0m plain"), "red plain");
        assert_eq!(
            strip_ansi("\u{1b}]8;;https://x\u{7}link\u{1b}]8;;\u{1b}\\"),
            "link"
        );
        assert_eq!(strip_ansi("trailing\u{1b}"), "trailing");
    }
}
