//! TypeSafe's Jev API: typed multiple-choice answers about a text state.
//! Called through curl (Windows' built-in curl.exe with schannel TLS, the system curl
//! on Linux), so no TLS stack is compiled in. The key is passed on curl's stdin
//! config, never on a command line.

use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

#[cfg(windows)]
fn curl_path() -> String {
    std::env::var("SystemRoot").map(|r| format!(r"{r}\System32\curl.exe")).unwrap_or("curl.exe".into())
}

#[cfg(not(windows))]
fn curl_path() -> String {
    "curl".into()
}
use std::time::Duration;

pub const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// The endpoint, or a hidden override from `GPUTREE_JEV_URL` (for testing the offline
/// fallback against an unreachable address without touching the network).
pub fn endpoint() -> String {
    std::env::var("GPUTREE_JEV_URL").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| ENDPOINT.to_string())
}

pub struct Question {
    pub key: String,
    pub instructions: String,
    /// option id -> description
    pub criteria: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct Answer {
    pub choice: String,
    pub confidence: f64,
}

pub fn api_key() -> Option<String> {
    ["JEV_API_KEY", "TYPESAFE_API_KEY"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
}

fn curl_quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// One batched request. Returns question key -> answer, or an error string.
pub fn ask(key: &str, state: &str, questions: &[Question], timeout: Duration) -> Result<HashMap<String, Answer>, String> {
    let mut qs = Map::new();
    for q in questions {
        let crit: Map<String, Value> = q.criteria.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        qs.insert(q.key.clone(), json!({"type": "choice", "instructions": q.instructions, "criteria": crit}));
    }
    let body = json!({"state": state, "model": "jev-latest", "questions": qs}).to_string();
    let secs = timeout.as_secs_f64().max(0.5);
    // An unreachable host should give up fast so the local headline shows.
    let conn = secs.min(1.0);
    let config = format!(
        "url = {}\nheader = {}\nheader = \"Content-Type: application/json\"\ndata-binary = {}\nconnect-timeout = {conn:.1}\nmax-time = {secs:.1}\nsilent\n",
        curl_quote(&endpoint()),
        curl_quote(&format!("Authorization: Bearer {}", key.trim())),
        curl_quote(&body),
    );
    let mut child = crate::no_window(
        Command::new(curl_path()).args(["-K", "-", "-w", "\n%{http_code}"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()),
    )
    .spawn()
    .map_err(|e| format!("curl: {e}"))?;
    child.stdin.take().ok_or("curl stdin")?.write_all(config.as_bytes()).map_err(|e| format!("curl stdin: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (payload, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    if code.trim() != "200" {
        let code = if code.trim() == "000" { "timeout / network" } else { code.trim() };
        return Err(format!("HTTP {code}"));
    }
    let v: Value = serde_json::from_str(payload).map_err(|e| format!("bad json: {e}"))?;
    let answers = v.get("answers").and_then(Value::as_object).ok_or("no answers")?;
    Ok(answers
        .iter()
        .filter_map(|(k, a)| {
            Some((
                k.clone(),
                Answer {
                    choice: a.get("choice")?.as_str()?.to_string(),
                    confidence: a.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
                },
            ))
        })
        .collect())
}

/// One prepared request: the state text, the questions, and which process name each
/// `tag_N` question was about.
pub struct JevJob {
    pub state: String,
    pub questions: Vec<Question>,
    pub asked: HashMap<String, String>,
}

/// What came back: the chosen headline kind, and name (lowercase) -> tag re-labels.
#[derive(Clone, Debug, Default)]
pub struct JevResult {
    pub kind: Option<String>,
    pub tags: HashMap<String, &'static str>,
}

impl JevJob {
    /// Blocking; run it on a thread.
    pub fn run(self, key: &str, timeout: Duration) -> Result<JevResult, String> {
        let ans = ask(key, &self.state, &self.questions, timeout)?;
        let mut out = JevResult::default();
        for (k, a) in ans {
            if k == "headline" {
                out.kind = Some(a.choice);
            } else if let (Some(name), Some(tag)) = (self.asked.get(&k), crate::tags::intern(&a.choice)) {
                if tag != "other" && a.confidence >= 0.5 {
                    out.tags.insert(name.clone(), tag);
                }
            }
        }
        Ok(out)
    }
}

/// A "which tag is this process" question over the fixed tag list.
pub fn tag_question(key: String, instructions: String) -> Question {
    Question {
        key,
        instructions,
        criteria: crate::tags::ALL.iter().map(|t| (t.to_string(), crate::tags::describe(t).to_string())).collect(),
    }
}
