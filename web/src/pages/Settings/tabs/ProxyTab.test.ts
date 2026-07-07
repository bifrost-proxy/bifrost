import { describe, expect, it } from "vitest";
import { getCliInstallActionState } from "./ProxyTab";
import type { CliInstallStatus } from "../../../api/system";

function cliStatus(
  override: Partial<CliInstallStatus> = {},
): CliInstallStatus {
  return {
    installed: false,
    install_path: "",
    install_dir: "",
    current_exe: "",
    in_path: false,
    path_hint: null,
    skills_installed: null,
    skills_message: null,
    dry_run: false,
    ...override,
  };
}

describe("getCliInstallActionState", () => {
  it("offers CLI installation before a CLI is present", () => {
    expect(getCliInstallActionState(null)).toEqual({
      showInstallCli: true,
      showInstallSkills: false,
      skillsButtonLabel: "Install AI Skills",
    });

    expect(getCliInstallActionState(cliStatus({ installed: false }))).toEqual({
      showInstallCli: true,
      showInstallSkills: false,
      skillsButtonLabel: "Install AI Skills",
    });
  });

  it("offers skills installation only after the CLI is present", () => {
    expect(getCliInstallActionState(cliStatus({ installed: true }))).toEqual({
      showInstallCli: false,
      showInstallSkills: true,
      skillsButtonLabel: "Install AI Skills",
    });
  });

  it("keeps skills repair separate from CLI installation", () => {
    expect(
      getCliInstallActionState(
        cliStatus({ installed: true, skills_installed: true }),
      ),
    ).toEqual({
      showInstallCli: false,
      showInstallSkills: true,
      skillsButtonLabel: "Reinstall AI Skills",
    });
  });
});
