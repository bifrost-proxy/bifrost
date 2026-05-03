import { describe, expect, it } from "vitest";
import { buildSkillMd, type SkillFormValues } from "./SkillCreatorWizard";
import type { SkillManifest } from "./types";

function manifest(): SkillManifest {
  return {
    name: "test-skill",
    version: "0.1.0",
    description: "test skill",
    scope: "repo",
    entrypoint: { kind: "inline", instructions_md: "" },
    allowed_tools: [],
    slash_command: null,
    triggers: [{ kind: "description_match" }],
    inputs_schema: null,
    outputs_schema: null,
    metadata: {},
    created_by: { user: { id: "test" } },
    created_at_unix: 1,
    updated_at_unix: 1,
    checksum: "",
    schema_version: 1,
  };
}

function values(script: string): SkillFormValues {
  return {
    name: "test-skill",
    version: "0.1.0",
    description: "test skill",
    scope: "repo",
    entrypoint_kind: "inline",
    script,
    shell: "bash",
    inputs_schema: "{}",
    test_inputs: "{}",
  };
}

describe("buildSkillMd", () => {
  it("keeps frontmatter intact when script contains triple backticks", () => {
    const output = buildSkillMd(manifest(), values("```bash\necho hi\n```"));

    expect(output.match(/^---$/gm)).toHaveLength(2);
    expect(output).toContain("~~~");
    expect(output).toContain("```bash");
  });

  it("keeps one frontmatter closing marker when script starts with dashes", () => {
    const output = buildSkillMd(manifest(), values("---\necho hi"));

    expect(output.match(/^---$/gm)).toHaveLength(2);
    expect(output).toContain("~~~\n ---\necho hi\n~~~");
  });
});
