use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Java detail contract. Attempts are deliberately not counted or inspected.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Submission {
    pub submission_id: String,
    pub submitted_at: String,
    pub outcome: String,
    pub resolved_channel: Option<String>,
    pub payload: Value,
}

impl Submission {
    pub fn text(&self, field: &str) -> Option<&str> {
        self.payload.as_object()?.get(field)?.as_str()
    }
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    UnresolvedTemplate,
    SmsUnicode,
    SmsMultipart,
}

#[derive(Debug, Serialize)]
pub struct Finding {
    pub submission_id: String,
    pub rule: Rule,
    pub severity: Severity,
    pub field: &'static str,
    pub explanation: String,
}
