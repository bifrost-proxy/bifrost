---
name: skill-creator
description: Create or update a Bifrost Agent skill. Use this skill when the user wants to create a new skill (or update an existing skill) to extend the agent's capabilities with specialized knowledge, workflows, or domain-specific guidance. MANDATORY tool for creating SKILLs - MUST be invoked IMMEDIATELY when user wants to create/add any skill.
metadata:
  short-description: Create or update a skill
---

# Skill Creator

This skill guides you through creating effective Bifrost Agent skills.

## About Skills

Skills are modular, self-contained folders that extend the agent's capabilities by providing specialized knowledge, workflows, and domain-specific guidance. They transform the agent from a general-purpose assistant into a specialized expert.

### Skill Directory Structure

```
skill-name/
├── SKILL.md          (required) — frontmatter metadata + instructions
└── references/       (optional) — documentation loaded on demand
    └── *.md
```

### SKILL.md Format

```markdown
---
name: my-skill
description: What this skill does and WHEN to trigger it. Be specific.
---

# Skill Title

Instructions for the agent when this skill is active.
```

**Frontmatter rules:**
- `name`: kebab-case, max 64 characters
- `description`: the primary trigger mechanism; include what it does AND when to use it; max 1024 characters

## Storage Paths

Skills are discovered from these locations (higher priority first):

| Scope | Path | Use case |
|-------|------|----------|
| **Repo** | `<work_dir>/.agents/skills/<name>/` | Project-specific skills |
| **User** | `~/.bifrost/agent/skills/<name>/` | Personal skills across projects |
| **System** | `~/.bifrost/agent/skills/.system/<name>/` | Built-in read-only skills |

When the user does not specify a scope, default to **Repo** for project-specific skills, or **User** for personal/reusable skills.

## Creation Process

### Step 1: Understand the skill

Clarify with the user:
- What task or domain does this skill address?
- What should trigger the skill? (keywords, scenarios, explicit invocation)
- Where to store it? (repo or user scope)

### Step 2: Write SKILL.md

1. Choose a kebab-case name (e.g. `lark-doc`, `code-review`)
2. Write a clear `description` that covers **what** it does and **when** to trigger it
3. Write the body with concise, actionable instructions for the agent

**Keep SKILL.md under 500 lines.** Move large reference material to `references/*.md` files and link them from SKILL.md.

### Step 3: Commit the skill

Use `write_file` to create the SKILL.md at the correct path:

**Repo scope:**
```
<work_dir>/.agents/skills/<skill-name>/SKILL.md
```

**User scope:**
```
~/.bifrost/agent/skills/<skill-name>/SKILL.md
```

Create parent directories with `shell` if needed:
```bash
mkdir -p <path>
```

### Step 4: Validate

After writing, verify:
1. The file exists at the correct path
2. The frontmatter YAML is valid (name + description present)
3. Confirm with the user: "Skill `<name>` created at `<path>`"

## Writing Guidelines

- **Description is the trigger**: Write it so the agent can decide when to load this skill without reading the body
- **Body is the guide**: Write concise, imperative instructions for when the skill is active
- **Avoid redundancy**: Don't duplicate what the agent already knows
- **Progressive disclosure**: Keep SKILL.md lean; link to `references/` files for details
- **No extra docs**: Do NOT create README.md, CHANGELOG.md, or other auxiliary files

## Example: Creating a "git-workflow" skill

```bash
mkdir -p /path/to/project/.agents/skills/git-workflow
```

```markdown
---
name: git-workflow
description: Team git workflow guide. Use when the user asks about branching strategy, commit conventions, PR process, or code review workflow for this project.
---

# Git Workflow

## Branch Naming
- Features: `feat/<ticket>-<short-desc>`
- Fixes: `fix/<ticket>-<short-desc>`

## Commit Convention
Follow Conventional Commits: `type(scope): message`

## PR Process
1. Create feature branch from `main`
2. Open draft PR early for visibility
3. Request review when ready; 1 approval required
4. Squash merge to main
```
