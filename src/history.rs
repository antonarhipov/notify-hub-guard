use crate::model::Submission;
use crate::{Error, Result};
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;

pub fn valid_submission_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

pub struct HistoryClient {
    base: Url,
    client: Client,
    max_records: usize,
}

impl HistoryClient {
    pub fn new(base: &str, max_records: usize) -> Result<Self> {
        let base = Url::parse(base).map_err(|_| "Invalid base URL")?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(
                "Base URL must use HTTP(S), without credentials, query, or fragment".into(),
            );
        }
        if max_records == 0 {
            return Err("Record limit must be positive".into());
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .redirect(Policy::none())
            .user_agent("notify-hub-guard/0.1")
            .build()?;
        Ok(Self {
            base,
            client,
            max_records,
        })
    }

    fn endpoint(&self, id: Option<&str>) -> Url {
        let mut url = self.base.clone();
        let mut path = url
            .path_segments_mut()
            .expect("HTTP URLs have path segments");
        path.pop_if_empty().push("api").push("submissions");
        if let Some(id) = id {
            path.push(id);
        }
        drop(path);
        url
    }

    fn get<T: DeserializeOwned>(&self, url: Url) -> Result<T> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/json")
            .send()
            .map_err(|error| -> Error {
                if error.is_timeout() {
                    "Java history request timed out".into()
                } else {
                    "Cannot read Java history; check the URL and that NotifyHub is running".into()
                }
            })?;
        if !response.status().is_success() {
            return Err(
                format!("Java history returned HTTP {}", response.status().as_u16()).into(),
            );
        }
        let mut data = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|_| "Cannot read Java history response")?;
        if data.len() as u64 > MAX_RESPONSE_BYTES {
            return Err("Java history response exceeds 2 MiB".into());
        }
        serde_json::from_slice(&data)
            .map_err(|_| "Invalid Java history JSON or detail contract".into())
    }

    pub fn detail(&self, id: &str) -> Result<Submission> {
        if !valid_submission_id(id) {
            return Err("Submission ID must be a UUID".into());
        }
        let record: Submission = self.get(self.endpoint(Some(id)))?;
        if record.submission_id != id {
            return Err(
                "Java history detail identifier does not match the requested submission".into(),
            );
        }
        Ok(record)
    }

    pub fn load(&self) -> Result<Vec<Submission>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Item {
            submission_id: String,
        }
        let mut ids = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut seen_cursors = HashSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut url = self.endpoint(None);
            url.query_pairs_mut().append_pair("limit", "200");
            if let Some(cursor) = &cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            }
            let page: Value = self.get(url)?;
            let items: Vec<Item> = serde_json::from_value(
                page.get("items")
                    .cloned()
                    .ok_or("History page lacks items")?,
            )
            .map_err(|_| "Invalid history items")?;
            for item in &items {
                if !valid_submission_id(&item.submission_id) {
                    return Err("Invalid submission ID in history page".into());
                }
                if !seen_ids.insert(item.submission_id.clone()) {
                    return Err(
                        "Duplicate submission in history; retry on an idle Java instance".into(),
                    );
                }
                ids.push(item.submission_id.clone());
                if ids.len() > self.max_records {
                    return Err(format!(
                        "History exceeds the {}-submission limit; no partial report was written",
                        self.max_records
                    )
                    .into());
                }
            }
            cursor = match page.get("nextCursor") {
                Some(Value::Null) => break,
                Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
                _ => return Err("History page has a missing or invalid nextCursor".into()),
            };
            if items.is_empty() {
                return Err("Empty history page has a continuation cursor".into());
            }
            if !seen_cursors.insert(cursor.clone()) {
                return Err("Repeated history cursor".into());
            }
        }
        ids.iter().map(|id| self.detail(id)).collect()
    }
}

pub fn load_file(path: &Path, max_records: usize) -> Result<Vec<Submission>> {
    let data = std::fs::read(path)?;
    let records: Vec<Submission> = serde_json::from_slice(&data)
        .map_err(|_| "Input must be a JSON array of Java submission detail objects")?;
    if records.len() > max_records {
        return Err("Input exceeds the submission limit".into());
    }
    let mut ids = HashSet::new();
    for record in &records {
        if !valid_submission_id(&record.submission_id) {
            return Err("Input submission ID must be a UUID".into());
        }
        if !ids.insert(&record.submission_id) {
            return Err("Duplicate submission in input".into());
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_base_urls_and_context_path() {
        for url in [
            "file:///tmp/history",
            "http://user:pass@localhost",
            "http://localhost?x=1",
            "http://localhost#fragment",
            "broken",
        ] {
            assert!(HistoryClient::new(url, 5).is_err(), "{url}");
        }
        let client = HistoryClient::new("http://localhost:8080/demo/", 5).unwrap();
        assert_eq!(
            client.endpoint(None).as_str(),
            "http://localhost:8080/demo/api/submissions"
        );
        assert!(HistoryClient::new("http://localhost", 0).is_err());
    }

    #[test]
    fn rejects_path_injection_as_submission_id() {
        assert!(valid_submission_id("10000000-0000-0000-0000-000000000001"));
        for id in ["../notify", "", "10000000-0000-0000-0000-00000000000z"] {
            assert!(!valid_submission_id(id));
        }
    }
}
