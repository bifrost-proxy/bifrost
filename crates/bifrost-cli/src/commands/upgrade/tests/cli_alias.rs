use crate::cli::{Cli, Commands};
use clap::{CommandFactory, Parser};

#[test]
fn rejects_removed_restart_flag_for_both_spellings() {
    for command in ["upgrade", "update"] {
        let result = Cli::try_parse_from(["bifrost", command, "--restart"]);
        assert!(
            result.is_err(),
            "--restart should be removed from {command}"
        );
    }
}

#[test]
fn accepts_hidden_yes_flag_for_both_spellings() {
    for command in ["upgrade", "update"] {
        let cli = Cli::parse_from(["bifrost", command, "-y"]);
        match cli.command {
            Some(Commands::Upgrade { yes, local_assets }) => {
                assert!(yes, "{command} should preserve the hidden yes flag");
                assert!(local_assets.is_none());
            }
            _ => panic!("Expected {command} to parse as Upgrade command"),
        }
    }
}

#[test]
fn uses_same_default_flags_for_both_spellings() {
    for command in ["upgrade", "update"] {
        let cli = Cli::parse_from(["bifrost", command]);
        match cli.command {
            Some(Commands::Upgrade { yes, local_assets }) => {
                assert!(!yes, "{command} should use the same default flags");
                assert!(local_assets.is_none());
            }
            _ => panic!("Expected {command} to parse as Upgrade command"),
        }
    }
}

#[test]
fn accepts_local_assets_for_both_spellings() {
    for command in ["upgrade", "update"] {
        let cli = Cli::parse_from(["bifrost", command, "--local-assets", "C:/bifrost-assets"]);
        match cli.command {
            Some(Commands::Upgrade { yes, local_assets }) => {
                assert!(!yes);
                assert_eq!(
                    local_assets.as_deref(),
                    Some(std::path::Path::new("C:/bifrost-assets"))
                );
            }
            _ => panic!("Expected {command} to parse as Upgrade command"),
        }
    }
}

#[test]
fn top_level_help_advertises_update_alias() {
    let help = Cli::command().render_help().to_string();
    assert!(
        help.contains("alias: update"),
        "top-level help should make the update alias discoverable: {help}"
    );
}
