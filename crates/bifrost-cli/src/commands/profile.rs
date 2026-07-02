use bifrost_core::profile::{
    analyze_compatibility, convert_surge_to_bifrost_preview, explain_surge_request,
    parse_surge_profile_file, CompatibilityReport, ConversionPreview, ExplainReport,
    ProfileDocument, SupportLevel,
};

use crate::cli::{ProfileCommands, ProfileConvertTarget};

pub fn handle_profile_command(action: ProfileCommands) -> bifrost_core::Result<()> {
    match action {
        ProfileCommands::Import {
            profile,
            dry_run,
            json,
        } => {
            if !dry_run {
                return Err(bifrost_core::BifrostError::Config(
                    "active profile import is not implemented yet; rerun with --dry-run"
                        .to_string(),
                ));
            }
            let document = parse_surge_profile_file(&profile)?;
            let report = analyze_compatibility(&document);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_import_report(&document, &report);
            }
        }
        ProfileCommands::Explain {
            profile,
            target,
            json,
        } => {
            let document = parse_surge_profile_file(&profile)?;
            let report = explain_surge_request(&document, &target)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_explain_report(&report);
            }
        }
        ProfileCommands::Convert { profile, to, json } => {
            let document = parse_surge_profile_file(&profile)?;
            let preview = match to {
                ProfileConvertTarget::Bifrost => convert_surge_to_bifrost_preview(&document),
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&preview)?);
            } else {
                print_conversion_preview(&preview);
            }
        }
    }

    Ok(())
}

fn print_import_report(document: &ProfileDocument, report: &CompatibilityReport) {
    println!("Surge profile dry-run import");
    println!("Source: {}", source_label(document));
    println!("Sections: {}", document.sections.len());
    println!(
        "Compatibility: {} fully supported, {} translated with behavior note, {} needs manual review, {} not supported yet",
        report.summary.fully_supported,
        report.summary.translated_with_behavior_note,
        report.summary.needs_manual_review,
        report.summary.not_supported_yet,
    );

    if !report.diagnostics.is_empty() {
        println!();
        println!("Diagnostics:");
        for diagnostic in &report.diagnostics {
            println!(
                "  {:?} line {}:{} [{}] {}",
                diagnostic.severity,
                diagnostic.line,
                diagnostic.column,
                diagnostic.code,
                diagnostic.message
            );
        }
    }

    if !report.items.is_empty() {
        println!();
        println!("Compatibility items:");
        for item in &report.items {
            println!(
                "  line {:>4} [{:<31}] {:<24} {}",
                item.line,
                support_label(item.level),
                item.capability,
                item.message
            );
            if let Some(suggestion) = &item.suggestion {
                println!("       suggestion: {suggestion}");
            }
        }
    }
}

fn print_explain_report(report: &ExplainReport) {
    println!("Surge profile explain");
    println!("URL: {}", report.request.url);
    println!("Host: {}", report.request.host);
    match (&report.matched_rule, &report.target_policy) {
        (Some(rule), Some(policy)) => {
            println!(
                "Matched: line {} {} -> {}",
                rule.source.line, rule.rule_type, policy
            );
        }
        _ => println!("Matched: none"),
    }

    println!();
    println!("Decision timeline:");
    for step in &report.timeline {
        match step.line {
            Some(line) => println!("  {} line {}: {}", step.stage, line, step.message),
            None => println!("  {}: {}", step.stage, step.message),
        }
    }

    if !report.diagnostics.is_empty() {
        println!();
        println!("Diagnostics:");
        for diagnostic in &report.diagnostics {
            println!(
                "  {:?} line {} [{}] {}",
                diagnostic.severity, diagnostic.line, diagnostic.code, diagnostic.message
            );
        }
    }
}

fn print_conversion_preview(preview: &ConversionPreview) {
    println!("{}", preview.content);
    println!(
        "# Compatibility summary: {} fully supported, {} translated with behavior note, {} needs manual review, {} not supported yet",
        preview.report.summary.fully_supported,
        preview.report.summary.translated_with_behavior_note,
        preview.report.summary.needs_manual_review,
        preview.report.summary.not_supported_yet,
    );
}

fn source_label(document: &ProfileDocument) -> String {
    match &document.source {
        bifrost_core::profile::ProfileSource::LocalPath(path) => path.display().to_string(),
        bifrost_core::profile::ProfileSource::ManagedUrl(url) => url.clone(),
        bifrost_core::profile::ProfileSource::Inline => "<inline>".to_string(),
    }
}

fn support_label(level: SupportLevel) -> &'static str {
    match level {
        SupportLevel::FullySupported => "Fully supported",
        SupportLevel::TranslatedWithBehaviorNote => "Translated with behavior note",
        SupportLevel::NeedsManualReview => "Needs manual review",
        SupportLevel::NotSupportedYet => "Not supported yet",
    }
}
