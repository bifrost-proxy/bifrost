//! `$SkillName` mention parsing and on-demand injection.
//!
//! Provides extraction of `$skill-name` sigil mentions and `[$name](path)` links
//! from user text, and resolves them against loaded skills for on-demand injection.

use super::{parse_frontmatter, SkillMetadata};
use std::collections::{HashMap, HashSet};

/// Represents parsed skill mentions from user text.
#[derive(Debug, Default)]
pub struct SkillMentions<'a> {
    /// Skill names mentioned via `$skill-name` sigil.
    pub names: HashSet<&'a str>,
    /// Explicit file paths from `[$name](path)` links.
    pub paths: HashSet<&'a str>,
}

/// The sigil character used to reference skills in user text.
const SKILL_MENTION_SIGIL: u8 = b'$';

/// Extract `$skill-name` mentions and `[$skill-name](path)` links from text.
///
/// Matches Codex's `extract_tool_mentions` behavior.
pub fn extract_skill_mentions(text: &str) -> SkillMentions<'_> {
    let bytes = text.as_bytes();
    let mut names: HashSet<&str> = HashSet::new();
    let mut paths: HashSet<&str> = HashSet::new();

    let mut i = 0;
    while i < bytes.len() {
        // Check for linked mention: [$name](path)
        if bytes[i] == b'[' {
            if let Some((name, path, end)) = parse_linked_mention(text, bytes, i) {
                names.insert(name);
                paths.insert(path);
                i = end;
                continue;
            }
        }

        // Check for $sigil mention
        if bytes[i] == SKILL_MENTION_SIGIL {
            let name_start = i + 1;
            if name_start < bytes.len() && is_mention_name_char(bytes[name_start]) {
                let mut name_end = name_start + 1;
                while name_end < bytes.len() && is_mention_name_char(bytes[name_end]) {
                    name_end += 1;
                }
                let name = &text[name_start..name_end];
                if !is_common_env_var(name) {
                    names.insert(name);
                }
                i = name_end;
                continue;
            }
        }

        i += 1;
    }

    SkillMentions { names, paths }
}

/// Parse `[$name](path)` link format.
fn parse_linked_mention<'a>(
    text: &'a str,
    bytes: &[u8],
    start: usize,
) -> Option<(&'a str, &'a str, usize)> {
    // Expect [$
    let sigil_idx = start + 1;
    if bytes.get(sigil_idx) != Some(&SKILL_MENTION_SIGIL) {
        return None;
    }

    // Parse name
    let name_start = sigil_idx + 1;
    if name_start >= bytes.len() || !is_mention_name_char(bytes[name_start]) {
        return None;
    }
    let mut name_end = name_start + 1;
    while name_end < bytes.len() && is_mention_name_char(bytes[name_end]) {
        name_end += 1;
    }

    // Expect ]
    if bytes.get(name_end) != Some(&b']') {
        return None;
    }

    // Skip whitespace, expect (
    let mut path_paren = name_end + 1;
    while path_paren < bytes.len() && bytes[path_paren].is_ascii_whitespace() {
        path_paren += 1;
    }
    if bytes.get(path_paren) != Some(&b'(') {
        return None;
    }

    // Find closing )
    let mut path_end = path_paren + 1;
    while path_end < bytes.len() && bytes[path_end] != b')' {
        path_end += 1;
    }
    if bytes.get(path_end) != Some(&b')') {
        return None;
    }

    let path = text[path_paren + 1..path_end].trim();
    if path.is_empty() {
        return None;
    }

    let name = &text[name_start..name_end];
    Some((name, path, path_end + 1))
}

fn is_mention_name_char(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b':')
}

fn is_common_env_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "PATH"
            | "HOME"
            | "USER"
            | "SHELL"
            | "PWD"
            | "TMPDIR"
            | "TEMP"
            | "TMP"
            | "LANG"
            | "TERM"
            | "XDG_CONFIG_HOME"
    )
}

/// Resolve mentioned skill names/paths against the loaded skills.
///
/// Returns skills that should have their full SKILL.md content injected into the current turn.
pub fn resolve_mentioned_skills<'a>(
    mentions: &SkillMentions<'_>,
    skills: &'a [SkillMetadata],
) -> Vec<&'a SkillMetadata> {
    if mentions.names.is_empty() && mentions.paths.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut seen_names: HashSet<&str> = HashSet::new();

    // First pass: resolve by path (exact match)
    for skill in skills {
        let path_str = skill.path.to_string_lossy();
        if mentions.paths.contains(path_str.as_ref()) && seen_names.insert(&skill.name) {
            result.push(skill);
        }
    }

    // Second pass: resolve by name (unambiguous match only)
    let name_counts: HashMap<&str, usize> = skills.iter().fold(HashMap::new(), |mut map, s| {
        *map.entry(s.name.as_str()).or_insert(0) += 1;
        map
    });

    for skill in skills {
        if seen_names.contains(skill.name.as_str()) {
            continue;
        }
        if mentions.names.contains(skill.name.as_str()) {
            // Only resolve if name is unambiguous
            if name_counts.get(skill.name.as_str()).copied().unwrap_or(0) == 1 {
                seen_names.insert(&skill.name);
                result.push(skill);
            }
        }
    }

    result
}

/// Build injection text for explicitly mentioned skills (read their full SKILL.md).
pub fn build_skill_injections(mentioned_skills: &[&SkillMetadata]) -> String {
    if mentioned_skills.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    for skill in mentioned_skills {
        output.push_str(&format!(
            "\n## Skill: {} ({})\n",
            skill.name,
            skill.path.display()
        ));
        if !skill.prompt_content.is_empty() {
            output.push_str(&skill.prompt_content);
            output.push('\n');
        } else {
            // Try to read from disk
            match std::fs::read_to_string(&skill.path) {
                Ok(content) => {
                    let (_, body) = parse_frontmatter(&content);
                    output.push_str(body);
                    output.push('\n');
                }
                Err(e) => {
                    output.push_str(&format!("(Failed to load skill: {e})\n"));
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillScope;
    use std::path::PathBuf;

    fn make_skill(name: &str, path: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: format!("Description for {name}"),
            short_description: None,
            prompt_content: format!("Content for {name}"),
            path: PathBuf::from(path),
            scope: SkillScope::Repo,
            interface: None,
            dependencies: None,
            policy: None,
        }
    }

    #[test]
    fn test_extract_sigil_mention() {
        let text = "Please use $my-skill to do this";
        let mentions = extract_skill_mentions(text);
        assert!(mentions.names.contains("my-skill"));
        assert!(mentions.paths.is_empty());
    }

    #[test]
    fn test_extract_multiple_mentions() {
        let text = "Use $alpha and $beta-skill together";
        let mentions = extract_skill_mentions(text);
        assert!(mentions.names.contains("alpha"));
        assert!(mentions.names.contains("beta-skill"));
    }

    #[test]
    fn test_extract_linked_mention() {
        let text = "Use [$my-skill](/path/to/SKILL.md) for this";
        let mentions = extract_skill_mentions(text);
        assert!(mentions.names.contains("my-skill"));
        assert!(mentions.paths.contains("/path/to/SKILL.md"));
    }

    #[test]
    fn test_ignores_common_env_vars() {
        let text = "Set $PATH and $HOME correctly";
        let mentions = extract_skill_mentions(text);
        assert!(mentions.names.is_empty());
    }

    #[test]
    fn test_colon_in_name() {
        let text = "Use $ns:skill-name here";
        let mentions = extract_skill_mentions(text);
        assert!(mentions.names.contains("ns:skill-name"));
    }

    #[test]
    fn test_resolve_by_name() {
        let skills = vec![
            make_skill("alpha", "/skills/alpha/SKILL.md"),
            make_skill("beta", "/skills/beta/SKILL.md"),
        ];
        let mentions = extract_skill_mentions("Use $alpha for this");
        let resolved = resolve_mentioned_skills(&mentions, &skills);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "alpha");
    }

    #[test]
    fn test_resolve_by_path() {
        let skills = vec![
            make_skill("alpha", "/skills/alpha/SKILL.md"),
            make_skill("beta", "/skills/beta/SKILL.md"),
        ];
        let mentions = extract_skill_mentions("Use [$beta](/skills/beta/SKILL.md) here");
        let resolved = resolve_mentioned_skills(&mentions, &skills);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "beta");
    }

    #[test]
    fn test_resolve_ambiguous_name_skipped() {
        let skills = vec![
            make_skill("dup", "/skills/dup1/SKILL.md"),
            make_skill("dup", "/skills/dup2/SKILL.md"),
        ];
        let mentions = extract_skill_mentions("Use $dup");
        let resolved = resolve_mentioned_skills(&mentions, &skills);
        // Ambiguous name should not resolve
        assert!(resolved.is_empty());
    }

    #[test]
    fn test_resolve_empty_mentions() {
        let skills = [make_skill("alpha", "/skills/alpha/SKILL.md")];
        let mentions = SkillMentions::default();
        let resolved = resolve_mentioned_skills(&mentions, &skills);
        assert!(resolved.is_empty());
    }

    #[test]
    fn test_build_injections_with_content() {
        let skills = [make_skill("alpha", "/skills/alpha/SKILL.md")];
        let refs: Vec<&SkillMetadata> = skills.iter().collect();
        let output = build_skill_injections(&refs);
        assert!(output.contains("## Skill: alpha"));
        assert!(output.contains("Content for alpha"));
    }

    #[test]
    fn test_build_injections_empty() {
        let output = build_skill_injections(&[]);
        assert!(output.is_empty());
    }
}
