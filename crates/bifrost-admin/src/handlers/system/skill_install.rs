use std::path::{Path, PathBuf};

use super::{DesktopInstallSkillStatus, DESKTOP_INSTALL_SKILL_TIMEOUT};

pub(super) fn install_result_fields(
    install_result: std::io::Result<DesktopInstallSkillStatus>,
    detected_skills_installed: Option<bool>,
) -> (Option<bool>, Option<String>) {
    match install_result {
        Ok(DesktopInstallSkillStatus::Success) => {
            let skills_installed = detected_skills_installed.unwrap_or(false);
            let message = if skills_installed {
                "Bifrost AI skills installed from embedded desktop bundle".to_string()
            } else {
                "install-skill completed, but the full AI skill bundle could not be verified; retry with `bifrost install-skill --tool all -y`".to_string()
            };
            (Some(skills_installed), Some(message))
        }
        Ok(DesktopInstallSkillStatus::Failed(message)) => (
            Some(false),
            Some(format!(
                "{message}; retry with `bifrost install-skill --tool all -y`"
            )),
        ),
        Ok(DesktopInstallSkillStatus::TimedOut) => (
            Some(false),
            Some(format!(
                "install-skill timed out after {}s; retry with `bifrost install-skill --tool all -y`",
                DESKTOP_INSTALL_SKILL_TIMEOUT.as_secs()
            )),
        ),
        Err(error) => (
            Some(false),
            Some(format!(
                "install-skill failed: {error}; retry with `bifrost install-skill --tool all -y`"
            )),
        ),
    }
}

pub(super) fn status_message(skills_installed: Option<bool>) -> Option<String> {
    (skills_installed == Some(true)).then(|| "Bifrost AI skills are installed".to_string())
}

pub(super) fn current_installed() -> Option<bool> {
    let home = dirs::home_dir();
    let override_dir = std::env::var_os("BIFROST_INSTALL_SKILL_DIR").map(PathBuf::from);
    installed(home.as_deref(), override_dir.as_deref())
}

fn installed(home: Option<&Path>, override_dir: Option<&Path>) -> Option<bool> {
    let skill_files = if let Some(primary_dir) = override_dir {
        let sibling_root = primary_dir.parent().unwrap_or_else(|| Path::new("."));
        vec![
            primary_dir.join("SKILL.md"),
            sibling_root.join("bifrost-remote").join("SKILL.md"),
        ]
    } else {
        let home = home?;
        [home.join(".agents/skills"), home.join(".claude/skills")]
            .into_iter()
            .flat_map(|root| {
                ["bifrost", "bifrost-remote"]
                    .into_iter()
                    .map(move |skill| root.join(skill).join("SKILL.md"))
            })
            .collect()
    };

    Some(skill_files.iter().all(|path| path.is_file()))
}

#[cfg(test)]
mod tests {
    use super::{install_result_fields, installed, status_message};
    use crate::handlers::system::DesktopInstallSkillStatus;

    #[test]
    fn installed_requires_the_complete_bundle() {
        let home = tempfile::tempdir().expect("temp home");
        let universal_root = home.path().join(".agents/skills");
        let claude_root = home.path().join(".claude/skills");

        assert_eq!(installed(Some(home.path()), None), Some(false));
        assert_eq!(installed(None, None), None);

        for root in [&universal_root, &claude_root] {
            for skill in ["bifrost", "bifrost-remote"] {
                let skill_dir = root.join(skill);
                std::fs::create_dir_all(&skill_dir).expect("create skill dir");
                std::fs::write(skill_dir.join("SKILL.md"), "---\nname: test\n---\n")
                    .expect("write skill");
            }
        }

        assert_eq!(installed(Some(home.path()), None), Some(true));

        std::fs::remove_file(claude_root.join("bifrost-remote/SKILL.md"))
            .expect("remove one skill");
        assert_eq!(installed(Some(home.path()), None), Some(false));
    }

    #[test]
    fn installed_honors_the_installer_override_dir() {
        let root = tempfile::tempdir().expect("temp skill root");
        let primary = root.path().join("bifrost");
        let remote = root.path().join("bifrost-remote");
        std::fs::create_dir_all(&primary).expect("create primary skill dir");
        std::fs::create_dir_all(&remote).expect("create remote skill dir");
        std::fs::write(primary.join("SKILL.md"), "primary").expect("write primary skill");
        std::fs::write(remote.join("SKILL.md"), "remote").expect("write remote skill");

        assert_eq!(installed(None, Some(primary.as_path())), Some(true));
    }

    #[test]
    fn install_success_requires_verified_files() {
        let (installed, message) =
            install_result_fields(Ok(DesktopInstallSkillStatus::Success), Some(true));
        assert_eq!(installed, Some(true));
        assert_eq!(
            message.as_deref(),
            Some("Bifrost AI skills installed from embedded desktop bundle")
        );

        let (installed, message) =
            install_result_fields(Ok(DesktopInstallSkillStatus::Success), None);
        assert_eq!(installed, Some(false));
        assert!(message
            .as_deref()
            .unwrap_or_default()
            .contains("could not be verified"));
        assert!(message
            .as_deref()
            .unwrap_or_default()
            .contains("bifrost install-skill --tool all -y"));

        let (installed, message) =
            install_result_fields(Ok(DesktopInstallSkillStatus::TimedOut), Some(true));
        assert_eq!(installed, Some(false));
        assert!(message
            .as_deref()
            .unwrap_or_default()
            .contains("timed out after 20s"));

        let (installed, message) =
            install_result_fields(Err(std::io::Error::other("spawn failed")), Some(true));
        assert_eq!(installed, Some(false));
        assert!(message
            .as_deref()
            .unwrap_or_default()
            .contains("install-skill failed: spawn failed"));

        assert_eq!(
            status_message(Some(true)).as_deref(),
            Some("Bifrost AI skills are installed")
        );
        assert_eq!(status_message(Some(false)), None);
        assert_eq!(status_message(None), None);
    }
}
