//! Budget-aware skill rendering for progressive disclosure in system prompts.
//!
//! Instead of injecting the full SKILL.md body, only outputs name + description + file path.
//! The model can read the SKILL.md via `read_file` when it decides to use a skill.
//! Descriptions are truncated proportionally when the budget is tight.

use super::{SkillMetadata, SkillScope};

/// Budget for skill metadata in the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillMetadataBudget {
    /// Budget expressed in approximate token count.
    Tokens(usize),
    /// Budget expressed in character count.
    Characters(usize),
}

/// Approximate bytes per token for budget calculation.
const APPROX_BYTES_PER_TOKEN: usize = 4;
/// Default character budget when context window is unknown.
const DEFAULT_SKILL_METADATA_CHAR_BUDGET: usize = 8_000;
/// Percentage of context window allocated to skill metadata.
const SKILL_METADATA_CONTEXT_WINDOW_PERCENT: usize = 2;

impl SkillMetadataBudget {
    fn limit(self) -> usize {
        match self {
            Self::Tokens(limit) | Self::Characters(limit) => limit,
        }
    }

    fn cost(self, text: &str) -> usize {
        match self {
            Self::Tokens(_) => text
                .len()
                .saturating_add(APPROX_BYTES_PER_TOKEN - 1)
                .saturating_div(APPROX_BYTES_PER_TOKEN),
            Self::Characters(_) => text.chars().count(),
        }
    }
}

/// Compute the default skill metadata budget from context window size.
pub fn default_skill_metadata_budget(context_window: Option<i64>) -> SkillMetadataBudget {
    context_window
        .and_then(|window| usize::try_from(window).ok())
        .filter(|window| *window > 0)
        .map(|window| {
            SkillMetadataBudget::Tokens(
                window
                    .saturating_mul(SKILL_METADATA_CONTEXT_WINDOW_PERCENT)
                    .saturating_div(100)
                    .max(1),
            )
        })
        .unwrap_or(SkillMetadataBudget::Characters(
            DEFAULT_SKILL_METADATA_CHAR_BUDGET,
        ))
}

/// Report on skill rendering budget usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRenderReport {
    pub total_count: usize,
    pub included_count: usize,
    pub omitted_count: usize,
    pub truncated_description_chars: usize,
    /// Number of skills whose descriptions were truncated.
    pub truncated_description_count: usize,
}

pub(crate) const SKILLS_INTRO: &str = "A skill is a set of local instructions stored in a `SKILL.md` file. Below is the list of skills available. Each entry includes a name, description, and file path so you can open the source for full instructions when using a specific skill.\n";

pub(crate) const SKILLS_HOW_TO_USE: &str = r#"- Discovery: The list above shows skills available in this session (name + description + file path). Skill bodies live on disk at the listed paths.
- Trigger rules: If the user names a skill (with `$SkillName` or plain text) OR the task clearly matches a skill's description, you must use that skill for that turn. Multiple mentions mean use them all. Do not carry skills across turns unless re-mentioned.
- Missing/blocked: If a named skill isn't in the list or the path can't be read, say so briefly and continue with the best fallback.
- How to use a skill (progressive disclosure):
  1) After deciding to use a skill, open its `SKILL.md` via read_file. Read only enough to follow the workflow.
  2) When `SKILL.md` references relative paths (e.g., `scripts/foo.py`), resolve them relative to the skill directory listed above first, and only consider other paths if needed.
  3) If `SKILL.md` points to extra folders such as `references/`, load only the specific files needed for the request; don't bulk-load everything.
  4) If `scripts/` exist, prefer running or patching them instead of retyping large code blocks.
  5) If `assets/` or templates exist, reuse them instead of recreating from scratch.
- Coordination and sequencing:
  - If multiple skills apply, choose the minimal set that covers the request and state the order you'll use them.
  - Announce which skill(s) you're using and why (one short line). If you skip an obvious skill, say why.
- Context hygiene:
  - Keep context small: summarize long sections instead of pasting them; only load extra files when needed.
  - Avoid deep reference-chasing: prefer opening only files directly linked from `SKILL.md` unless you're blocked.
  - When variants exist (frameworks, providers, domains), pick only the relevant reference file(s) and note that choice.
- Safety and fallback: If a skill can't be applied cleanly (missing files, unclear instructions), state the issue, pick the next-best approach, and continue."#;

/// Get the default character budget constant.
pub(crate) fn default_char_budget() -> usize {
    DEFAULT_SKILL_METADATA_CHAR_BUDGET
}

/// Render skill lines with budget-aware truncation (Codex-aligned 3-tier strategy).
///
/// Skills are sorted by scope priority (System > Global > User > Repo) then by name,
/// so that higher-priority skills are preserved first when budget is tight.
pub fn render_skill_lines(
    skills: &[SkillMetadata],
    budget: SkillMetadataBudget,
) -> (Vec<String>, SkillRenderReport) {
    let total_count = skills.len();

    // Sort by scope priority (System first, Repo last) then by name.
    // Matches Codex's `ordered_skills_for_budget` behavior.
    let mut ordered: Vec<&SkillMetadata> = skills.iter().collect();
    ordered.sort_by(|a, b| {
        scope_priority_rank(&a.scope)
            .cmp(&scope_priority_rank(&b.scope))
            .then_with(|| a.name.cmp(&b.name))
    });

    // Build full lines: "- name: description (file: path)"
    let full_lines: Vec<String> = ordered
        .iter()
        .map(|s| {
            format!(
                "- {}: {} (file: {})",
                s.name,
                s.description,
                s.path.display()
            )
        })
        .collect();

    // Check if all fit within budget
    let full_cost: usize = full_lines
        .iter()
        .map(|l| budget.cost(&format!("{l}\n")))
        .sum();
    if full_cost <= budget.limit() {
        return (
            full_lines,
            SkillRenderReport {
                total_count,
                included_count: total_count,
                omitted_count: 0,
                truncated_description_chars: 0,
                truncated_description_count: 0,
            },
        );
    }

    // Tier 2: Truncate descriptions proportionally
    let minimum_lines: Vec<String> = ordered
        .iter()
        .map(|s| format!("- {}: (file: {})", s.name, s.path.display()))
        .collect();
    let minimum_cost: usize = minimum_lines
        .iter()
        .map(|l| budget.cost(&format!("{l}\n")))
        .sum();

    if minimum_cost <= budget.limit() {
        let remaining = budget.limit().saturating_sub(minimum_cost);
        let ratio = if full_cost > minimum_cost {
            (remaining as f64) / (full_cost.saturating_sub(minimum_cost) as f64)
        } else {
            0.0
        };
        let ratio = ratio.min(1.0);

        let mut truncated_chars = 0usize;
        let mut truncated_count = 0usize;
        let lines: Vec<String> = ordered
            .iter()
            .map(|s| {
                let desc_len = s.description.chars().count();
                let allowed = ((desc_len as f64) * ratio).floor() as usize;
                let truncated = desc_len.saturating_sub(allowed);
                if truncated > 0 {
                    truncated_chars += truncated;
                    truncated_count += 1;
                }
                if allowed == 0 {
                    format!("- {}: (file: {})", s.name, s.path.display())
                } else {
                    let prefix: String = s.description.chars().take(allowed).collect();
                    format!("- {}: {} (file: {})", s.name, prefix, s.path.display())
                }
            })
            .collect();

        return (
            lines,
            SkillRenderReport {
                total_count,
                included_count: total_count,
                omitted_count: 0,
                truncated_description_chars: truncated_chars,
                truncated_description_count: truncated_count,
            },
        );
    }

    // Tier 3: Omit skills that don't fit (scope-prioritized order preserved)
    let mut included = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;
    for line in &minimum_lines {
        let cost = budget.cost(&format!("{line}\n"));
        if used.saturating_add(cost) <= budget.limit() {
            used += cost;
            included.push(line.clone());
        } else {
            omitted += 1;
        }
    }
    let truncated_description_chars: usize =
        ordered.iter().map(|s| s.description.chars().count()).sum();
    let truncated_description_count = ordered.iter().filter(|s| !s.description.is_empty()).count();

    (
        included,
        SkillRenderReport {
            total_count,
            included_count: total_count - omitted,
            omitted_count: omitted,
            truncated_description_chars,
            truncated_description_count,
        },
    )
}

/// Scope priority rank for budget-aware ordering.
/// Lower rank = higher priority = kept first when budget is tight.
/// Matches Codex's `prompt_scope_rank` ordering:
///   System(0) > Admin/Global(1) > Repo(2) > User(3)
/// Repo skills are project-specific and more relevant than generic user skills.
fn scope_priority_rank(scope: &SkillScope) -> u8 {
    match scope {
        SkillScope::System => 0,
        SkillScope::Global => 1,
        SkillScope::Repo => 2,
        SkillScope::User => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillScope;
    use std::path::PathBuf;

    fn make_skill(name: &str, desc: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            description: desc.to_string(),
            short_description: None,
            prompt_content: String::new(),
            path: PathBuf::from(format!("/skills/{name}/SKILL.md")),
            scope: SkillScope::Repo,
            interface: None,
            dependencies: None,
            policy: None,
        }
    }

    #[test]
    fn test_full_lines_fit_within_budget() {
        let skills = vec![
            make_skill("a", "Alpha skill"),
            make_skill("b", "Beta skill"),
        ];
        let budget = SkillMetadataBudget::Characters(500);
        let (lines, report) = render_skill_lines(&skills, budget);
        assert_eq!(lines.len(), 2);
        assert_eq!(report.total_count, 2);
        assert_eq!(report.included_count, 2);
        assert_eq!(report.omitted_count, 0);
        assert_eq!(report.truncated_description_chars, 0);
        assert_eq!(report.truncated_description_count, 0);
        assert!(lines[0].contains("Alpha skill"));
    }

    #[test]
    fn test_truncation_when_tight() {
        let skills = vec![
            make_skill("alpha", "A very long description that takes space"),
            make_skill("beta", "Another long description occupying budget"),
        ];
        // Set budget tight enough to force truncation but not omission
        let budget = SkillMetadataBudget::Characters(100);
        let (lines, report) = render_skill_lines(&skills, budget);
        assert_eq!(report.total_count, 2);
        assert_eq!(report.included_count, 2);
        assert!(report.truncated_description_chars > 0);
        // Lines should be shorter than full descriptions
        for line in &lines {
            assert!(line.len() < 80);
        }
    }

    #[test]
    fn test_omission_when_very_tight() {
        let skills = vec![
            make_skill("alpha", "desc"),
            make_skill("beta", "desc"),
            make_skill("gamma", "desc"),
        ];
        // Budget so tight only 1-2 skills fit
        let budget = SkillMetadataBudget::Characters(40);
        let (lines, report) = render_skill_lines(&skills, budget);
        assert!(report.omitted_count > 0);
        assert!(lines.len() < 3);
    }

    #[test]
    fn test_default_budget_from_context_window() {
        let budget = default_skill_metadata_budget(Some(100_000));
        assert_eq!(budget, SkillMetadataBudget::Tokens(2_000));

        let budget = default_skill_metadata_budget(None);
        assert_eq!(
            budget,
            SkillMetadataBudget::Characters(DEFAULT_SKILL_METADATA_CHAR_BUDGET)
        );

        let budget = default_skill_metadata_budget(Some(0));
        assert_eq!(
            budget,
            SkillMetadataBudget::Characters(DEFAULT_SKILL_METADATA_CHAR_BUDGET)
        );
    }

    #[test]
    fn test_token_budget_cost() {
        let b = SkillMetadataBudget::Tokens(100);
        // "hello" = 5 bytes → ceil(5/4) = 2 tokens
        assert_eq!(b.cost("hello"), 2);
        // "hi" = 2 bytes → ceil(2/4) = 1 token
        assert_eq!(b.cost("hi"), 1);

        let b = SkillMetadataBudget::Characters(100);
        assert_eq!(b.cost("hello"), 5);
    }

    #[test]
    fn test_scope_priority_ordering_in_render() {
        // Codex ordering: System(0) > Global(1) > Repo(2) > User(3)
        let mut system_skill = make_skill("z-system", "System skill");
        system_skill.scope = SkillScope::System;
        let mut user_skill = make_skill("a-user", "User skill");
        user_skill.scope = SkillScope::User;
        let repo_skill = make_skill("b-repo", "Repo skill");

        // Pass in unsorted order
        let skills = vec![repo_skill, user_skill, system_skill];
        let budget = SkillMetadataBudget::Characters(500);
        let (lines, _) = render_skill_lines(&skills, budget);

        // System (rank 0) should come first, regardless of name ordering
        assert!(lines[0].contains("z-system"));
        // Repo (rank 2) second — project-specific skills are more relevant
        assert!(lines[1].contains("b-repo"));
        // User (rank 3) last — generic user skills are least important
        assert!(lines[2].contains("a-user"));
    }

    #[test]
    fn test_scope_priority_preserved_in_omission() {
        // When budget is tight, System skill should be kept over User skill
        let mut system_skill = make_skill("sys", "desc");
        system_skill.scope = SkillScope::System;
        let mut user_skill = make_skill("usr", "desc");
        user_skill.scope = SkillScope::User;

        // Calculate cost of one minimum line to calibrate budget
        let sys_min = "- sys: (file: /skills/sys/SKILL.md)\n".to_string();
        let budget_for_one = SkillMetadataBudget::Characters(10000).cost(&sys_min);
        // Set budget that fits exactly one minimum line
        let budget = SkillMetadataBudget::Characters(budget_for_one);
        let (lines, report) = render_skill_lines(&[user_skill, system_skill], budget);

        assert_eq!(report.omitted_count, 1);
        assert_eq!(lines.len(), 1);
        // System skill should be the one kept (higher priority than User)
        assert!(lines[0].contains("sys"));
    }
}
