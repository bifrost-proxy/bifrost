use crate::executor::{SkillExecutor, SkillInvocation, SkillTestReport};
use crate::model::{SkillManifest, SkillRecord};
use crate::store::{SkillDraft, SkillStore};
use crate::validator::SkillValidator;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthoringState {
    Started {
        session_id: String,
    },
    CapturedIntent {
        session_id: String,
        brief: String,
    },
    Interviewed {
        session_id: String,
        partial: serde_json::Value,
    },
    Drafted {
        session_id: String,
        draft_dir: PathBuf,
        skill_md: String,
    },
    Validated {
        session_id: String,
        draft_dir: PathBuf,
        manifest: SkillManifest,
    },
    Tested {
        session_id: String,
        draft_dir: PathBuf,
        test_report: SkillTestReport,
    },
    Committed {
        session_id: String,
        record: SkillRecord,
    },
    Cancelled {
        session_id: String,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthoringError {
    #[error("invalid authoring state: expected {expected}, actual {actual}")]
    InvalidState { expected: String, actual: String },
    #[error("execute skill: {0}")]
    Execute(String),
}

#[derive(Clone)]
pub struct SkillAuthoringSession {
    pub state: AuthoringState,
    store: Arc<SkillStore>,
    validator: SkillValidator,
}

impl SkillAuthoringSession {
    pub fn start(store: Arc<SkillStore>, brief: impl Into<String>) -> Self {
        let session_id = Uuid::new_v4().to_string();
        Self {
            state: AuthoringState::CapturedIntent {
                session_id,
                brief: brief.into(),
            },
            store,
            validator: SkillValidator::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        match &self.state {
            AuthoringState::Started { session_id }
            | AuthoringState::CapturedIntent { session_id, .. }
            | AuthoringState::Interviewed { session_id, .. }
            | AuthoringState::Drafted { session_id, .. }
            | AuthoringState::Validated { session_id, .. }
            | AuthoringState::Tested { session_id, .. }
            | AuthoringState::Committed { session_id, .. }
            | AuthoringState::Cancelled { session_id } => session_id,
        }
    }

    pub fn interview(&mut self, partial: serde_json::Value) {
        let session_id = self.session_id().to_string();
        self.state = AuthoringState::Interviewed {
            session_id,
            partial,
        };
    }

    pub fn draft(&mut self, draft_dir: PathBuf, skill_md: String) {
        let session_id = self.session_id().to_string();
        self.state = AuthoringState::Drafted {
            session_id,
            draft_dir,
            skill_md,
        };
    }

    pub fn validate(&mut self, manifest: SkillManifest, draft_dir: PathBuf) -> Result<(), String> {
        self.validator
            .validate_or_err(&manifest, Some(&draft_dir), Vec::new())
            .map_err(|error| format!("{:?}", error.issues))?;
        let session_id = self.session_id().to_string();
        self.state = AuthoringState::Validated {
            session_id,
            draft_dir,
            manifest,
        };
        Ok(())
    }

    pub async fn test(
        &mut self,
        record: &SkillRecord,
        inputs: serde_json::Value,
    ) -> Result<(), AuthoringError> {
        let draft_dir = match &self.state {
            AuthoringState::Validated { draft_dir, .. }
            | AuthoringState::Drafted { draft_dir, .. }
            | AuthoringState::Tested { draft_dir, .. } => draft_dir.clone(),
            actual => {
                return Err(AuthoringError::InvalidState {
                    expected: "validated, drafted, or tested".to_string(),
                    actual: actual.kind().to_string(),
                });
            }
        };
        let report = SkillExecutor::default()
            .execute(
                record,
                SkillInvocation {
                    input: inputs,
                    timeout_ms: None,
                },
            )
            .await
            .map_err(AuthoringError::Execute)?;
        let session_id = self.session_id().to_string();
        self.state = AuthoringState::Tested {
            session_id,
            draft_dir,
            test_report: report,
        };
        Ok(())
    }

    pub fn commit(&mut self, draft: SkillDraft) -> Result<SkillRecord, String> {
        let record = self
            .store
            .commit(draft)
            .map_err(|error| format!("commit skill: {error}"))?;
        let session_id = self.session_id().to_string();
        self.state = AuthoringState::Committed {
            session_id,
            record: record.clone(),
        };
        Ok(record)
    }

    pub fn cancel(&mut self) {
        let session_id = self.session_id().to_string();
        self.state = AuthoringState::Cancelled { session_id };
    }
}

impl AuthoringState {
    fn kind(&self) -> &'static str {
        match self {
            AuthoringState::Started { .. } => "started",
            AuthoringState::CapturedIntent { .. } => "captured_intent",
            AuthoringState::Interviewed { .. } => "interviewed",
            AuthoringState::Drafted { .. } => "drafted",
            AuthoringState::Validated { .. } => "validated",
            AuthoringState::Tested { .. } => "tested",
            AuthoringState::Committed { .. } => "committed",
            AuthoringState::Cancelled { .. } => "cancelled",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ScopeRoot, SkillManifest, SkillRecord, SkillScope};
    use tempfile::tempdir;

    #[test]
    fn state_machine_moves_through_interview_and_cancel() {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        let mut session = SkillAuthoringSession::start(store, "make weather");
        session.interview(serde_json::json!({"name":"weather"}));
        assert!(matches!(session.state, AuthoringState::Interviewed { .. }));
        session.cancel();
        assert!(matches!(session.state, AuthoringState::Cancelled { .. }));
    }

    #[tokio::test]
    async fn test_rejects_unvalidated_state() {
        let dir = tempdir().unwrap();
        let store = Arc::new(SkillStore::new(vec![ScopeRoot::new(
            SkillScope::Repo,
            dir.path(),
        )]));
        let manifest = SkillManifest::minimal_inline("weather", "weather", SkillScope::Repo);
        let record = SkillRecord {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            scope: SkillScope::Repo,
            effective_scope: SkillScope::Repo,
            shadow_scopes: Vec::new(),
            enabled: true,
            path: dir.path().into(),
            skill_md_path: dir.path().join("SKILL.md"),
            checksum: String::new(),
            manifest,
        };
        let mut session = SkillAuthoringSession::start(store, "make weather");

        let error = session
            .test(&record, serde_json::json!({}))
            .await
            .expect_err("unvalidated session must fail");
        assert!(matches!(error, AuthoringError::InvalidState { .. }));
    }
}
