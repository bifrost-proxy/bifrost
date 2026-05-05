#!/usr/bin/env python3
"""
Quick validation script for skills - validates structure, frontmatter, and naming.

Usage:
    quick_validate.py <skill_directory>
"""

import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:
    yaml = None

MAX_SKILL_NAME_LENGTH = 64


def parse_frontmatter(content):
    """Parse YAML frontmatter without requiring PyYAML (fallback to regex)."""
    if not content.startswith("---"):
        return None, "No YAML frontmatter found"

    match = re.match(r"^---\n(.*?)\n---", content, re.DOTALL)
    if not match:
        return None, "Invalid frontmatter format"

    frontmatter_text = match.group(1)

    if yaml:
        try:
            frontmatter = yaml.safe_load(frontmatter_text)
            if not isinstance(frontmatter, dict):
                return None, "Frontmatter must be a YAML dictionary"
            return frontmatter, None
        except yaml.YAMLError as e:
            return None, f"Invalid YAML in frontmatter: {e}"
    else:
        # Fallback: simple top-level key-value parsing (skip indented lines)
        frontmatter = {}
        for line in frontmatter_text.split("\n"):
            if not line or line.startswith("#") or line.startswith(" ") or line.startswith("\t"):
                continue
            if ":" in line:
                key, value = line.split(":", 1)
                frontmatter[key.strip()] = value.strip()
        return frontmatter, None


def validate_skill(skill_path):
    """Validate a skill directory structure and SKILL.md content."""
    skill_path = Path(skill_path)
    errors = []
    warnings = []

    # Check SKILL.md exists
    skill_md = skill_path / "SKILL.md"
    if not skill_md.exists():
        return False, ["SKILL.md not found"], []

    content = skill_md.read_text()

    # Parse frontmatter
    frontmatter, error = parse_frontmatter(content)
    if error:
        return False, [error], []

    # Check allowed properties
    allowed_properties = {"name", "description", "license", "allowed-tools", "metadata"}
    unexpected_keys = set(frontmatter.keys()) - allowed_properties
    if unexpected_keys:
        allowed = ", ".join(sorted(allowed_properties))
        unexpected = ", ".join(sorted(unexpected_keys))
        errors.append(
            f"Unexpected key(s) in frontmatter: {unexpected}. Allowed: {allowed}"
        )

    # Check required fields
    if "name" not in frontmatter:
        errors.append("Missing 'name' in frontmatter")
    if "description" not in frontmatter:
        errors.append("Missing 'description' in frontmatter")

    # Validate name
    name = frontmatter.get("name", "")
    if not isinstance(name, str):
        errors.append(f"Name must be a string, got {type(name).__name__}")
    else:
        name = name.strip()
        if name:
            if not re.match(r"^[a-z0-9-]+$", name):
                errors.append(
                    f"Name '{name}' should be hyphen-case (lowercase letters, digits, and hyphens only)"
                )
            if name.startswith("-") or name.endswith("-") or "--" in name:
                errors.append(
                    f"Name '{name}' cannot start/end with hyphen or contain consecutive hyphens"
                )
            if len(name) > MAX_SKILL_NAME_LENGTH:
                errors.append(
                    f"Name is too long ({len(name)} characters). Maximum is {MAX_SKILL_NAME_LENGTH}."
                )

    # Validate description
    description = frontmatter.get("description", "")
    if not isinstance(description, str):
        errors.append(f"Description must be a string, got {type(description).__name__}")
    else:
        description = description.strip()
        if description:
            if "<" in description or ">" in description:
                errors.append("Description cannot contain angle brackets (< or >)")
            if len(description) > 1024:
                errors.append(
                    f"Description is too long ({len(description)} characters). Maximum is 1024."
                )

    # Check body content
    body_match = re.match(r"^---\n.*?\n---\n(.*)", content, re.DOTALL)
    if body_match:
        body = body_match.group(1).strip()
        if not body:
            warnings.append("SKILL.md body is empty")
        else:
            line_count = len(body.split("\n"))
            if line_count > 500:
                warnings.append(
                    f"SKILL.md body is {line_count} lines (recommended max: 500). "
                    "Consider moving content to references/."
                )

    if errors:
        return False, errors, warnings
    return True, [], warnings


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print("Usage: python quick_validate.py <skill_directory>")
        sys.exit(1)

    skill_dir = Path(sys.argv[1])
    if not skill_dir.exists():
        print(f"[ERROR] Directory not found: {skill_dir}")
        sys.exit(1)
    if not skill_dir.is_dir():
        print(f"[ERROR] Path is not a directory: {skill_dir}")
        sys.exit(1)

    valid, errors, warnings = validate_skill(skill_dir)

    if errors:
        for error in errors:
            print(f"[ERROR] {error}")
    if warnings:
        for warning in warnings:
            print(f"[WARN] {warning}")

    if valid:
        print("[OK] Skill is valid!")
        sys.exit(0)
    else:
        sys.exit(1)
