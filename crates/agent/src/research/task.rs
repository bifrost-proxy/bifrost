use super::config::{ResearchSource, ResearchTaskConfig, ResearchTaskTrigger};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTaskRun {
    pub id: String,
    pub task_id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: ResearchTaskRunStatus,
    pub report_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchTaskRunStatus {
    Running,
    Success,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchTaskView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: ResearchTaskTrigger,
    pub queries: Vec<String>,
    pub sources: Vec<ResearchSource>,
    pub max_results_per_query: usize,
    pub fetch_content: bool,
}

impl From<&ResearchTaskConfig> for ResearchTaskView {
    fn from(task: &ResearchTaskConfig) -> Self {
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            enabled: task.enabled,
            trigger: task.trigger.clone(),
            queries: task.queries.clone(),
            sources: task.sources.clone(),
            max_results_per_query: task.max_results_per_query,
            fetch_content: task.fetch_content,
        }
    }
}
