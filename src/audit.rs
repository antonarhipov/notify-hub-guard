use crate::model::{Finding, Rule, Severity, Submission};
use crate::sms::{self, Encoding, Estimate};
use serde::Serialize;

pub trait AuditRule {
    fn check(&self, submission: &Submission) -> Vec<Finding>;
}

pub struct TemplateRule;
pub struct SmsRule;

// Recognize simple variable names, including dotted paths and hyphens, not expressions.
fn placeholder_count(text: &str, open: &str, close: &str) -> usize {
    let mut remaining = text;
    let mut count = 0;
    while let Some(start) = remaining.find(open) {
        let after_open = &remaining[start + open.len()..];
        let Some(end) = after_open.find(close) else {
            break;
        };
        let name = after_open[..end].trim();
        let mut characters = name.chars();
        if characters
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && characters.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        {
            count += 1;
            remaining = &after_open[end + close.len()..];
        } else {
            remaining = after_open;
        }
    }
    count
}

impl AuditRule for TemplateRule {
    fn check(&self, submission: &Submission) -> Vec<Finding> {
        ["subject", "body"]
            .into_iter()
            .filter_map(|field| {
                let text = submission.text(field)?;
                let count = placeholder_count(text, "{{", "}}")
                    + placeholder_count(text, "${", "}");
                (count > 0).then(|| Finding {
                    submission_id: submission.submission_id.clone(),
                    rule: Rule::UnresolvedTemplate,
                    severity: Severity::Warning,
                    field,
                    explanation: format!(
                        "{count} unresolved template placeholder(s) remain in the captured {field}. Replace variables before submission."
                    ),
                })
            })
            .collect()
    }
}

impl AuditRule for SmsRule {
    fn check(&self, submission: &Submission) -> Vec<Finding> {
        let Some(estimate) = sms_estimate(submission) else {
            return Vec::new();
        };
        let mut findings = Vec::new();
        if estimate.encoding == Encoding::Utf16 {
            findings.push(Finding {
                submission_id: submission.submission_id.clone(),
                rule: Rule::SmsUnicode,
                severity: Severity::Info,
                field: "body",
                explanation: format!(
                    "The body contains characters outside GSM-7. Its {} UTF-16 code units require an estimated {} SMS segment(s), using 70 units for a single segment or 67 per multipart segment. Supplementary characters use two code units each. Consider GSM-7 wording when segment cost matters.",
                    estimate.units, estimate.segments
                ),
            });
        }
        if estimate.segments > 1 {
            findings.push(Finding {
                submission_id: submission.submission_id.clone(),
                rule: Rule::SmsMultipart,
                severity: Severity::Warning,
                field: "body",
                explanation: format!(
                    "The body needs an estimated {} SMS segments ({} encoding units). Shorten the body to {} units for one segment.",
                    estimate.segments, estimate.units, estimate.single_segment_limit
                ),
            });
        }
        findings
    }
}

fn sms_estimate(submission: &Submission) -> Option<Estimate> {
    (submission.resolved_channel.as_deref() == Some("sms"))
        .then(|| submission.text("body").map(sms::estimate))
        .flatten()
}

#[derive(Debug, Serialize)]
pub struct SubmissionAudit {
    pub submission_id: String,
    pub submitted_at: String,
    pub outcome: String,
    pub resolved_channel: Option<String>,
    pub body_available: bool,
    pub sms: Option<Estimate>,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub submissions: usize,
    pub submissions_with_findings: usize,
    pub bodies_unavailable: usize,
    pub findings: usize,
    pub sms_submissions: usize,
    pub sms_bodies_audited: usize,
    pub estimated_sms_segments: usize,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub source: String,
    pub summary: Summary,
    pub submissions: Vec<SubmissionAudit>,
}

pub fn audit(submissions: &[Submission], source: String) -> Report {
    let rules: Vec<Box<dyn AuditRule>> = vec![Box::new(TemplateRule), Box::new(SmsRule)];
    let submissions: Vec<_> = submissions
        .iter()
        .map(|submission| SubmissionAudit {
            submission_id: submission.submission_id.clone(),
            submitted_at: submission.submitted_at.clone(),
            outcome: submission.outcome.clone(),
            resolved_channel: submission.resolved_channel.clone(),
            body_available: submission.text("body").is_some(),
            sms: sms_estimate(submission),
            findings: rules
                .iter()
                .flat_map(|rule| rule.check(submission))
                .collect(),
        })
        .collect();
    Report {
        schema_version: 1,
        source,
        summary: Summary {
            submissions: submissions.len(),
            submissions_with_findings: submissions
                .iter()
                .filter(|s| !s.findings.is_empty())
                .count(),
            bodies_unavailable: submissions.iter().filter(|s| !s.body_available).count(),
            findings: submissions.iter().map(|s| s.findings.len()).sum(),
            sms_submissions: submissions
                .iter()
                .filter(|s| s.resolved_channel.as_deref() == Some("sms"))
                .count(),
            sms_bodies_audited: submissions.iter().filter(|s| s.sms.is_some()).count(),
            estimated_sms_segments: submissions
                .iter()
                .filter_map(|s| s.sms.as_ref())
                .map(|s| s.segments)
                .sum(),
        },
        submissions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn submission(payload: serde_json::Value, channel: Option<&str>) -> Submission {
        Submission {
            submission_id: "demo".into(),
            submitted_at: "2026-10-19T10:00:00Z".into(),
            outcome: "ACCEPTED".into(),
            resolved_channel: channel.map(str::to_owned),
            payload,
        }
    }

    #[test]
    fn templates_in_both_fields_without_echoing_content() {
        let record = submission(
            json!({"subject": "Hello {{ firstName }}", "body": "${user.name}: {{order-id}} ${user.name}"}),
            Some("email"),
        );
        let findings = TemplateRule.check(&record);
        assert_eq!(findings.len(), 2);
        assert!(findings[1].explanation.starts_with("3 unresolved"));
        assert!(
            !serde_json::to_string(&findings)
                .unwrap()
                .contains("firstName")
        );
    }

    #[test]
    fn literal_braces_and_incomplete_expressions_are_not_variables() {
        for text in [
            "Hello Alice",
            "{name}",
            "{{}}",
            "${}",
            "{{ name",
            "{{a b}}",
            "${1}",
            "{{a + b}}",
            "漢 {{",
        ] {
            assert!(
                TemplateRule
                    .check(&submission(json!({"body": text}), None))
                    .is_empty(),
                "{text}"
            );
        }
        assert_eq!(placeholder_count("{{bad {{good}}", "{{", "}}"), 1);
    }

    #[test]
    fn no_requested_channel_fallback_or_attempt_double_counting() {
        let record = submission(json!({"channel": "sms", "body": "😀".repeat(80)}), None);
        let report = audit(&[record], "test".into());
        assert_eq!(report.summary.submissions, 1);
        assert_eq!(report.summary.sms_submissions, 0);
        assert!(report.submissions[0].findings.is_empty());
    }

    #[test]
    fn missing_or_invalid_content_is_explicit() {
        let records = vec![
            submission(serde_json::Value::Null, Some("sms")),
            submission(json!({"body": 123, "subject": "{{name}}"}), Some("sms")),
        ];
        let report = audit(&records, "test".into());
        assert_eq!(report.summary.bodies_unavailable, 2);
        assert_eq!(report.summary.sms_submissions, 2);
        assert_eq!(report.summary.sms_bodies_audited, 0);
        assert_eq!(report.summary.findings, 1);
    }

    #[test]
    fn segment_totals_include_clean_sms_and_preserve_outcomes() {
        let mut emoji = submission(json!({"body": "a".repeat(80) + "😀"}), Some("sms"));
        emoji.outcome = "FAILED".into();
        let report = audit(
            &[
                submission(json!({"body": "a".repeat(80)}), Some("sms")),
                emoji,
            ],
            "test".into(),
        );
        assert_eq!(report.summary.estimated_sms_segments, 3);
        assert_eq!(report.summary.findings, 2);
        assert_eq!(report.submissions[1].outcome, "FAILED");
    }
}
