use super::store::KnowledgeSearchResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchDigestRequest {
    pub task_id: Option<String>,
    pub date: Option<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub query: Option<String>,
}

fn default_format() -> String {
    "markdown".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchDigestResponse {
    pub report_path: String,
    pub items_used: usize,
    pub summary: String,
}

pub fn write_markdown_report(
    reports_root: &Path,
    task_id: &str,
    date: &str,
    query: Option<&str>,
    items: &[KnowledgeSearchResult],
) -> anyhow::Result<ResearchDigestResponse> {
    let dir = reports_root.join(task_id);
    std::fs::create_dir_all(&dir)?;
    let report_path = dir.join(format!("{date}.md"));
    let summary = build_summary(items);
    let markdown = build_markdown(task_id, date, query, items, &summary);
    let tmp = tmp_path(&report_path);
    std::fs::write(&tmp, markdown)?;
    std::fs::rename(&tmp, &report_path)?;
    Ok(ResearchDigestResponse {
        report_path: report_path.display().to_string(),
        items_used: items.len(),
        summary,
    })
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    tmp.set_extension("tmp");
    tmp
}

fn build_summary(items: &[KnowledgeSearchResult]) -> String {
    if items.is_empty() {
        return "本次没有找到可用于生成报告的资料。".to_string();
    }
    let first_titles = items
        .iter()
        .take(3)
        .map(|item| item.title.as_str())
        .collect::<Vec<_>>()
        .join("、");
    format!(
        "本次使用 {} 条资料，重点包括：{}。",
        items.len(),
        first_titles
    )
}

fn build_markdown(
    task_id: &str,
    date: &str,
    query: Option<&str>,
    items: &[KnowledgeSearchResult],
    summary: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} - {}\n\n", task_id, date));
    out.push_str("## TL;DR\n\n");
    out.push_str(&format!("- {}\n\n", summary));
    out.push_str("## 重点发现\n\n");
    if items.is_empty() {
        out.push_str("- 暂无资料。\n\n");
    } else {
        for (idx, item) in items.iter().enumerate() {
            out.push_str(&format!("### {}. {}\n\n", idx + 1, item.title));
            out.push_str("来源：\n");
            out.push_str(&format!("- [{}]({})\n\n", item.title, item.url));
            out.push_str("源信息：\n\n");
            out.push_str(&format!(
                "- source: `{}`\n- provider: `{}`\n- canonical_url: `{}`\n",
                item.source, item.provider, item.canonical_url
            ));
            if let Some(author) = &item.author {
                out.push_str(&format!("- author: `{author}`\n"));
            }
            if let Some(published_at) = &item.published_at {
                out.push_str(&format!("- published_at: `{published_at}`\n"));
            }
            out.push('\n');
            if let Some(summary) = &item.summary {
                out.push_str("摘要：\n\n");
                out.push_str(summary);
                out.push_str("\n\n");
            }
            if let Some(matched) = &item.matched_text {
                out.push_str("匹配片段：\n\n");
                out.push_str(matched);
                out.push_str("\n\n");
            }
        }
    }
    out.push_str("## 值得跟进\n\n");
    out.push_str("- 继续核对多来源之间是否存在事实冲突。\n");
    out.push_str("- 对高价值来源补充正文抓取和人工判断。\n\n");
    out.push_str("## 原始检索 Query\n\n");
    out.push_str(&format!("- {}\n\n", query.unwrap_or("(manual digest)")));
    out.push_str("## 本次使用来源\n\n");
    out.push_str("| 来源 | 数量 |\n|---|---:|\n");
    for (source, count) in source_counts(items) {
        out.push_str(&format!("| {} | {} |\n", source, count));
    }
    out
}

fn source_counts(items: &[KnowledgeSearchResult]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(item.source.clone()).or_insert(0) += 1;
    }
    if counts.is_empty() {
        counts.insert("knowledge".to_string(), 0);
    }
    counts
}
