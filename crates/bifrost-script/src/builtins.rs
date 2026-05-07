use crate::error::Result;
use std::path::Path;
use tracing::info;

pub const BUILD_IN_BP_SCRIPT_NAME: &str = "build_in_bp";

const BUILD_IN_BP_SCRIPT: &str = include_str!("../../../assets/scripts/parser/build_in_bp.js");

struct BuiltInParserScript {
    name: &'static str,
    content: &'static str,
}

const BUILT_IN_PARSER_SCRIPTS: &[BuiltInParserScript] = &[BuiltInParserScript {
    name: BUILD_IN_BP_SCRIPT_NAME,
    content: BUILD_IN_BP_SCRIPT,
}];

pub fn release_parser_scripts(parser_dir: &Path) -> Result<Vec<&'static str>> {
    std::fs::create_dir_all(parser_dir)?;

    let mut released = Vec::with_capacity(BUILT_IN_PARSER_SCRIPTS.len());
    for script in BUILT_IN_PARSER_SCRIPTS {
        let path = parser_dir.join(format!("{}.js", script.name));
        std::fs::write(&path, script.content)?;
        info!(
            script = script.name,
            path = %path.display(),
            "Released built-in parser script"
        );
        released.push(script.name);
    }

    Ok(released)
}

#[cfg(test)]
pub(crate) fn build_in_bp_script_content() -> &'static str {
    BUILD_IN_BP_SCRIPT
}
