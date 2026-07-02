use bifrost_core::profile::{
    analyze_compatibility, convert_resolved_surge_to_bifrost_preview,
    explain_surge_request_with_plan, load_surge_profile_path_or_url, CompatibilityReport,
    ConversionPreview, ExplainReport, ProfileDocument, ResolvedProfileDocument, SupportLevel,
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
            let resolved = load_surge_profile_path_or_url(&profile)?;
            let report = analyze_compatibility(&resolved.document);
            if json {
                println!("{}", serde_json::to_string_pretty(&resolved)?);
            } else {
                print_import_report(&resolved.document, &report);
                print_resource_summary(&resolved);
            }
        }
        ProfileCommands::Explain {
            profile,
            target,
            json,
        } => {
            let resolved = load_surge_profile_path_or_url(&profile)?;
            let report = explain_surge_request_with_plan(&resolved.runtime_plan, &target)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_explain_report(&report);
            }
        }
        ProfileCommands::Convert { profile, to, json } => {
            let resolved = load_surge_profile_path_or_url(&profile)?;
            let preview = match to {
                ProfileConvertTarget::Bifrost => {
                    convert_resolved_surge_to_bifrost_preview(&resolved)
                }
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&preview)?);
            } else {
                print_conversion_preview(&preview);
            }
        }
        ProfileCommands::Effective { profile, json } => {
            let resolved = load_surge_profile_path_or_url(&profile)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&resolved.runtime_plan)?);
            } else {
                print_effective_profile(&resolved);
            }
        }
    }

    Ok(())
}

fn print_resource_summary(resolved: &ResolvedProfileDocument) {
    if resolved.resources.is_empty() {
        return;
    }
    println!();
    println!("Resolved resources:");
    for resource in &resolved.resources {
        let cache_state = if resource.loaded_from_cache {
            "cache-hit"
        } else {
            "fresh"
        };
        println!(
            "  line {:>4} [{:?}] {:?} {} ({} items, {}, cache {})",
            resource.source_line,
            resource.status,
            resource.kind,
            resource.reference,
            resource.item_count,
            cache_state,
            resource.cache_key.as_deref().unwrap_or("<none>")
        );
        if resource.etag.is_some() || resource.last_modified.is_some() {
            println!(
                "       etag {} last-modified {}",
                resource.etag.as_deref().unwrap_or("<none>"),
                resource.last_modified.as_deref().unwrap_or("<none>")
            );
        }
    }
}

fn print_effective_profile(resolved: &ResolvedProfileDocument) {
    println!("Surge effective profile dry-run");
    println!("Source: {}", source_label(&resolved.document));
    println!("Mode: {}", resolved.runtime_plan.mode);
    println!(
        "Runtime plan: {} proxies, {} policy groups, {} rules, {} dns entries, {} mitm entries, {} pipeline entries",
        resolved.runtime_plan.proxies.len(),
        resolved.runtime_plan.policy_groups.len(),
        resolved.runtime_plan.rules.len(),
        resolved.runtime_plan.dns.len(),
        resolved.runtime_plan.mitm.len(),
        resolved.runtime_plan.http_pipeline.len(),
    );
    print_resource_summary(resolved);

    if !resolved.runtime_plan.policy_groups.is_empty() {
        println!();
        println!("Policy graph:");
        for group in &resolved.runtime_plan.policy_groups {
            println!(
                "  line {:>4} {} = {}, {}",
                group.source.line,
                group.name,
                group.group_type,
                group.policies.join(", ")
            );
            if !group.missing_members.is_empty() {
                println!(
                    "       missing members: {}",
                    group.missing_members.join(", ")
                );
            }
        }
    }

    if !resolved.runtime_plan.rules.is_empty() {
        println!();
        println!("Ordered rules:");
        for rule in &resolved.runtime_plan.rules {
            println!(
                "  line {:>4} {:<14} {:<32} -> {:<16} origin {}",
                rule.source.line,
                rule.rule_type,
                rule.value.as_deref().unwrap_or("<none>"),
                rule.policy,
                rule.origin
            );
        }
    }
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
    if let Some(decision) = &report.policy_decision {
        println!(
            "Policy decision: {} (terminal {}; {})",
            decision.chain.join(" -> "),
            decision.terminal_policy,
            decision.reason
        );
    }
    if let Some(mapping) = &report.dns_decision.matched_host_mapping {
        println!("DNS decision: {mapping}");
    } else if let Some(note) = report.dns_decision.notes.first() {
        println!("DNS decision: {note}");
    }
    println!("MITM decision: {}", report.mitm_decision.reason);
    let matched_pipeline = report
        .http_pipeline
        .iter()
        .filter(|entry| entry.matched)
        .count();
    if !report.http_pipeline.is_empty() {
        println!(
            "HTTP pipeline: {} matched / {} total",
            matched_pipeline,
            report.http_pipeline.len()
        );
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
