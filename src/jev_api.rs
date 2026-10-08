//! The external Jev API: the relevance decision over retrieval candidates.
//!
//! Jev is [TypeSafe AI's System One decision model](https://docs.typesafe.ai)
//! — an external service, not local code. contextleleo stays the source of
//! truth for session history and performs the cheap local candidate search;
//! Jev answers the expensive question — *given this task and these candidate
//! chunks, which ones actually matter?* The retrieval pipeline is:
//!
//! ```text
//! local candidate retrieval (dozens of chunks)
//!         │  JevClient::rank — state + one noul question per candidate
//!   TypeSafe Jev API — a probability per candidate
//!         │  probability ≥ RETRIEVE_THRESHOLD ⇒ retrieve, else ignore
//!         ▼
//! existing Jev optimizer (keep / compress / drop to the budget)
//! ```
//!
//! The wire format is `TypeSafe`'s System One schema:
//!
//! ```text
//! POST https://api.typesafe.ai/v1/systemone      (JEV_API_URL overrides)
//! Authorization: Bearer $JEV_API_KEY             (or $TYPESAFE_API_KEY)
//! { "model": "jev-latest", "state": "…task + numbered candidate excerpts…",
//!   "questions": { "candidate_1": { "type": "noul", "instructions": …, "criteria": … } } }
//! → { "model": "jev-1.13.0",
//!     "answers": { "candidate_1": { "type": "noul", "noul": 0.69 } },
//!     "usage": { "input_tokens": 434, "output_tokens": 40 } }
//! ```
//!
//! Configuration is environment only, so the key never reaches transcripts,
//! logs, or the repository: [`API_KEY_ENV`] (or its [`API_KEY_ALIAS_ENV`]
//! alias) is required; [`API_URL_ENV`] and [`MODEL_ENV`] have documented
//! defaults. Without a key, [`JevClient::from_env`] fails with a
//! configuration error and only the Jev-powered retrieval paths stop —
//! every other command runs with no key at all.
//!
//! Only candidates leave this crate: a bounded excerpt per chunk, never the
//! whole history (see [`excerpt`]).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Error;

/// Environment variable carrying the Jev (`TypeSafe`) API key. The primary,
/// contextleleo-specific name.
pub const API_KEY_ENV: &str = "JEV_API_KEY";

/// `TypeSafe`'s own environment variable name for the same key, accepted as
/// a fallback so an existing TypeSafe/SDK setup works unchanged.
pub const API_KEY_ALIAS_ENV: &str = "TYPESAFE_API_KEY";

/// Environment variable overriding the ranking endpoint.
pub const API_URL_ENV: &str = "JEV_API_URL";

/// Environment variable overriding the Jev model id.
pub const MODEL_ENV: &str = "JEV_MODEL";

/// The `TypeSafe` System One endpoint, used when [`API_URL_ENV`] is unset.
pub const DEFAULT_API_URL: &str = "https://api.typesafe.ai/v1/systemone";

/// The Jev model used when [`MODEL_ENV`] is unset; the vendor's alias that
/// tracks the newest release (`jev-1.13` as of writing).
pub const DEFAULT_MODEL: &str = "jev-latest";

/// Probability at or above which Jev's answer counts as `retrieve`. Jev
/// returns calibrated probabilities; 0.5 is the neutral cut, and raising it
/// trades context breadth for precision.
pub const RETRIEVE_THRESHOLD: f32 = 0.5;

/// Error surface in [`crate::Error::Remote`]: every Jev call failure names
/// this harness so remote Jev errors never blend into harness errors.
const HARNESS: &str = "jev";

/// Longest candidate content sent to Jev, in characters — enough for a
/// relevance judgment, small enough to keep each request cheap and to send
/// as little history as possible off the machine.
const CANDIDATE_CONTENT_CHARS: usize = 800;

/// Hard cap on a Jev response body, in bytes.
const MAX_RESPONSE_BYTES: usize = 1 << 20;

/// How long one ranking call may take before it fails.
const TIMEOUT: Duration = Duration::from_mins(1);

/// Criterion text telling Jev what counts as "yes" for a candidate.
const CRITERION_TRUE: &str =
    "The chunk contains facts, decisions, code, errors, or configuration that help with the task.";

/// Criterion text telling Jev what counts as "no" for a candidate.
const CRITERION_FALSE: &str =
    "The chunk is unrelated, redundant, or too generic to help with the task.";

/// One candidate chunk as sent to Jev: its source locator and an excerpt.
///
/// The `id` is the `session#message` locator the rest of the crate prints,
/// so a decision maps straight back onto the authoritative local original.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevCandidate {
    /// `session#message` locator — the same id Jev answers with.
    pub id: String,
    /// Excerpt of the chunk's text (truncated by [`excerpt`]).
    pub content: String,
}

/// What Jev decided for one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevDecisionKind {
    /// The candidate matters for the task; carry it forward.
    Retrieve,
    /// The candidate does not matter for the task; leave it out.
    Ignore,
}

/// Jev's answer for one candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevDecision {
    /// The candidate's `session#message` id.
    pub id: String,
    /// Jev's probability that the candidate helps with the task, `0..=1`.
    pub relevance: f32,
    /// `Retrieve` at or above [`RETRIEVE_THRESHOLD`], `Ignore` below.
    pub decision: JevDecisionKind,
}

/// The System One request body.
#[derive(Debug, Serialize)]
struct RankRequest<'a> {
    /// The Jev model id.
    model: &'a str,
    /// The task plus every candidate excerpt, numbered `[n]`.
    state: String,
    /// One `noul` question per candidate, keyed `candidate_1`…`candidate_n`.
    questions: BTreeMap<String, NoulQuestion>,
}

/// One `noul` (yes/no) question in System One's schema.
#[derive(Debug, Serialize)]
struct NoulQuestion {
    /// Always `noul` — the primitive whose answer is a probability.
    #[serde(rename = "type")]
    kind: &'static str,
    /// What Jev is judging, naming the candidate's place in the state.
    instructions: String,
    /// What "yes" and "no" mean for this judgment.
    criteria: NoulCriteria,
}

/// The `true`/`false` descriptions of a `noul` question.
#[derive(Debug, Serialize)]
struct NoulCriteria {
    /// What a `true` answer looks like.
    #[serde(rename = "true")]
    yes: &'static str,
    /// What a `false` answer looks like.
    #[serde(rename = "false")]
    no: &'static str,
}

/// The System One response body.
#[derive(Debug, Deserialize)]
struct AnswerResponse {
    /// One answer per question name.
    answers: BTreeMap<String, Answer>,
}

/// One typed answer; only the `noul` field matters here.
#[derive(Debug, Deserialize)]
struct Answer {
    /// The primitive that answered (`noul` for this client).
    #[serde(rename = "type")]
    kind: String,
    /// The probability of "yes", when the answer is a `noul`.
    #[serde(default)]
    noul: Option<f32>,
}

/// A client for the external Jev (`TypeSafe` System One) ranking API.
///
/// Holds the key, endpoint, and model; the key is redacted from [`Debug`]
/// so it can never leak through logging, and it is sent only as a sensitive
/// `Authorization` header.
#[derive(Clone)]
pub struct JevClient {
    key: String,
    url: String,
    model: String,
    timeout: Duration,
}

impl std::fmt::Debug for JevClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JevClient")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("key", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl JevClient {
    /// A client for `url` authenticated with `key`, using [`DEFAULT_MODEL`].
    #[must_use]
    pub fn new(key: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            url: url.into(),
            model: DEFAULT_MODEL.to_string(),
            timeout: TIMEOUT,
        }
    }

    /// Build a client from the environment.
    ///
    /// [`API_KEY_ENV`] (or [`API_KEY_ALIAS_ENV`]) is required and must be
    /// non-blank; [`API_URL_ENV`] and [`MODEL_ENV`] fall back to
    /// [`DEFAULT_API_URL`] and [`DEFAULT_MODEL`].
    ///
    /// # Errors
    ///
    /// [`Error::JevNotConfigured`] naming both key variables, so a
    /// Jev-powered command stops with one actionable configuration error
    /// instead of a mystery failure.
    pub fn from_env() -> crate::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// [`from_env`] over an injectable lookup — the env-independent core,
    /// so configuration behavior is testable without touching real
    /// environment variables.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> crate::Result<Self> {
        let non_blank = |name: &str| lookup(name).filter(|value| !value.trim().is_empty());
        let key = non_blank(API_KEY_ENV).or_else(|| non_blank(API_KEY_ALIAS_ENV));
        let Some(key) = key else {
            return Err(Error::JevNotConfigured(format!(
                "export {API_KEY_ENV} (or {API_KEY_ALIAS_ENV}) — get a key at \
                 https://docs.typesafe.ai/introduction/quickstart — to send retrieval \
                 candidates to Jev"
            )));
        };
        Ok(Self {
            key,
            url: non_blank(API_URL_ENV).unwrap_or_else(|| DEFAULT_API_URL.to_string()),
            model: non_blank(MODEL_ENV).unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            timeout: TIMEOUT,
        })
    }

    /// Ask Jev which of `candidates` matter for `query`.
    ///
    /// One `noul` question per candidate is judged against the same state
    /// (the task plus every excerpt); a probability at or above
    /// [`RETRIEVE_THRESHOLD`] is `Retrieve`, below it `Ignore`. An empty
    /// candidate set short-circuits without a network call.
    ///
    /// # Errors
    ///
    /// [`Error::Remote`] when the request cannot be made, Jev answers with
    /// a non-2xx status, an oversized body, malformed JSON, or an answer
    /// set that does not cover the candidates one-for-one.
    pub fn rank(
        &self,
        query: &str,
        candidates: &[JevCandidate],
    ) -> crate::Result<Vec<JevDecision>> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut questions = BTreeMap::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let number = index + 1;
            questions.insert(
                candidate_question(number),
                NoulQuestion {
                    kind: "noul",
                    instructions: format!(
                        "Candidate [{number}] ({}) in the state contains information that \
                         helps with the task.",
                        candidate.id
                    ),
                    criteria: NoulCriteria {
                        yes: CRITERION_TRUE,
                        no: CRITERION_FALSE,
                    },
                },
            );
        }
        let payload = serde_json::to_vec(&RankRequest {
            model: &self.model,
            state: build_state(query, candidates),
            questions,
        })?;
        let (status, body) = self.post(payload)?;
        if !(200..300).contains(&status) {
            return Err(status_error(status, &body));
        }
        let response: AnswerResponse =
            serde_json::from_slice(&body).map_err(|error| Error::Remote {
                harness: HARNESS,
                detail: format!("Jev returned unexpected JSON: {error}"),
            })?;
        decisions_from_answers(candidates, response.answers)
    }

    /// POST one System One request and read the response, capped.
    fn post(&self, payload: Vec<u8>) -> crate::Result<(u16, Vec<u8>)> {
        // Header first: a malformed key fails before any runtime spins up,
        // and the value is marked sensitive so no transport error can echo it.
        let mut auth = wreq::header::HeaderValue::from_str(&format!("Bearer {}", self.key))
            .map_err(|_| Error::Remote {
                harness: HARNESS,
                detail: format!("the {API_KEY_ENV} value is not a valid Authorization header"),
            })?;
        auth.set_sensitive(true);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Error::Remote {
                harness: HARNESS,
                detail: format!("could not start the Jev HTTP runtime: {error}"),
            })?;
        let client = wreq::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|error| Error::Remote {
                harness: HARNESS,
                detail: format!("could not build the Jev HTTP client: {error}"),
            })?;
        runtime.block_on(async {
            let response = client
                .post(&self.url)
                .header(wreq::header::CONTENT_TYPE, "application/json")
                .header(wreq::header::AUTHORIZATION, auth)
                .body(payload)
                .send()
                .await
                .map_err(|error| Error::Remote {
                    harness: HARNESS,
                    detail: format!("the Jev ranking request failed: {error}"),
                })?;
            let status = response.status().as_u16();
            if response.content_length().is_some_and(|length| {
                length > u64::try_from(MAX_RESPONSE_BYTES).unwrap_or(u64::MAX)
            }) {
                return Err(Error::Remote {
                    harness: HARNESS,
                    detail: format!("Jev's response exceeded {MAX_RESPONSE_BYTES} bytes"),
                });
            }
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                let chunk = chunk.map_err(|error| Error::Remote {
                    harness: HARNESS,
                    detail: format!("failed reading Jev's response: {error}"),
                })?;
                if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(Error::Remote {
                        harness: HARNESS,
                        detail: format!("Jev's response exceeded {MAX_RESPONSE_BYTES} bytes"),
                    });
                }
                body.extend_from_slice(&chunk);
            }
            Ok::<(u16, Vec<u8>), Error>((status, body))
        })
    }
}

/// The question name for the `number`-th candidate (1-based), stable across
/// requests so answers map back by position.
fn candidate_question(number: usize) -> String {
    format!("candidate_{number}")
}

/// The `state` string: the task, then each candidate numbered `[n]` with its
/// `session#message` locator — the exact layout the question instructions
/// refer to.
fn build_state(query: &str, candidates: &[JevCandidate]) -> String {
    use std::fmt::Write as _;
    let mut state = String::from("Task:\n");
    state.push_str(query.trim());
    state.push_str("\n\nCandidate chunks (numbered; each names its source):\n");
    for (index, candidate) in candidates.iter().enumerate() {
        let _ = writeln!(
            state,
            "\n[{}] {}\n{}",
            index + 1,
            candidate.id,
            candidate.content
        );
    }
    state
}

/// Turn Jev's `noul` answers into per-candidate decisions, requiring one
/// `noul` answer per candidate and no answers for anything else.
fn decisions_from_answers(
    candidates: &[JevCandidate],
    mut answers: BTreeMap<String, Answer>,
) -> crate::Result<Vec<JevDecision>> {
    let mut decisions = Vec::with_capacity(candidates.len());
    for (index, candidate) in candidates.iter().enumerate() {
        let name = candidate_question(index + 1);
        let Some(answer) = answers.remove(&name) else {
            return Err(Error::Remote {
                harness: HARNESS,
                detail: format!("Jev did not answer {name} for candidate {}", candidate.id),
            });
        };
        let Some(probability) = answer.noul else {
            return Err(Error::Remote {
                harness: HARNESS,
                detail: format!(
                    "Jev answered {name} with `{}`, expected a noul probability",
                    answer.kind
                ),
            });
        };
        if !probability.is_finite() {
            return Err(Error::Remote {
                harness: HARNESS,
                detail: format!("Jev answered {name} with a non-finite probability"),
            });
        }
        let relevance = probability.clamp(0.0, 1.0);
        let decision = if relevance >= RETRIEVE_THRESHOLD {
            JevDecisionKind::Retrieve
        } else {
            JevDecisionKind::Ignore
        };
        decisions.push(JevDecision {
            id: candidate.id.clone(),
            relevance,
            decision,
        });
    }
    if let Some(name) = answers.keys().next() {
        return Err(Error::Remote {
            harness: HARNESS,
            detail: format!("Jev answered unknown question {name}"),
        });
    }
    Ok(decisions)
}

/// Truncate `content` to [`CANDIDATE_CONTENT_CHARS`] on a character
/// boundary, marking the cut — all that a relevance decision needs.
pub(crate) fn excerpt(content: &str) -> String {
    let mut characters = content.chars();
    let mut text: String = characters.by_ref().take(CANDIDATE_CONTENT_CHARS).collect();
    if characters.next().is_some() {
        text.push_str("\n[truncated]");
    }
    text
}

/// The [`crate::Error::Remote`] for a non-2xx Jev status, with guidance
/// that names the configuration where it helps and quotes at most 160
/// characters of the server's own message (flattened to one line).
fn status_error(status: u16, body: &[u8]) -> Error {
    let guidance = match status {
        401 => format!("the key was rejected; check {API_KEY_ENV}"),
        402 => "the account is out of credit; check the TypeSafe/OpenRouter balance".to_string(),
        403 => format!("the key was refused; check its scope in {API_KEY_ENV}"),
        404 => format!("no Jev endpoint at that URL; check {API_URL_ENV}"),
        429 => "Jev rate-limited the ranking; retry shortly".to_string(),
        _ => "Jev rejected the ranking request".to_string(),
    };
    let flat: String = String::from_utf8_lossy(body)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let message: String = flat.chars().take(160).collect();
    let detail = if message.is_empty() {
        format!("HTTP {status}: {guidance}")
    } else {
        format!("HTTP {status}: {guidance}: {message}")
    };
    Error::Remote {
        harness: HARNESS,
        detail,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Serve exactly one HTTP request on loopback, capture what the
    /// client sent, and reply with `status` + `body`.
    fn serve_once(status: &str, body: String) -> (String, mpsc::Receiver<String>) {
        let status = status.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("local address");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            // Read headers, then exactly Content-Length bytes of body.
            loop {
                let read = stream.read(&mut buffer).expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(head_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
                let length = head
                    .split("content-length:")
                    .nth(1)
                    .and_then(|rest| rest.split("\r\n").next())
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= head_end + 4 + length {
                    break;
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        });
        (format!("http://{address}/v1/systemone"), receiver)
    }

    fn candidate(id: &str, content: &str) -> JevCandidate {
        JevCandidate {
            id: id.to_string(),
            content: content.to_string(),
        }
    }

    /// A System One answer body with one `noul` probability per entry.
    fn answers(probabilities: &[(&str, f32)]) -> String {
        let answers: BTreeMap<&str, serde_json::Value> = probabilities
            .iter()
            .map(|(name, probability)| {
                (
                    *name,
                    serde_json::json!({"type": "noul", "noul": probability}),
                )
            })
            .collect();
        serde_json::json!({
            "model": "jev-1.13.0",
            "answers": answers,
            "usage": {"input_tokens": 434, "output_tokens": 40},
        })
        .to_string()
    }

    #[test]
    fn from_env_reads_the_key_and_defaults_url_and_model() {
        let client = JevClient::from_lookup(|name| (name == API_KEY_ENV).then(|| "secret".into()))
            .expect("configured");
        assert_eq!(client.key, "secret");
        assert_eq!(client.url, DEFAULT_API_URL);
        assert_eq!(client.model, DEFAULT_MODEL);
    }

    #[test]
    fn from_env_accepts_the_typesafe_alias_and_overrides() {
        let alias = JevClient::from_lookup(|name| {
            (name == API_KEY_ALIAS_ENV).then(|| "typesafe-secret".to_string())
        })
        .expect("alias key");
        assert_eq!(alias.key, "typesafe-secret");

        let overridden = JevClient::from_lookup(|name| match name {
            API_KEY_ENV => Some("secret".to_string()),
            API_URL_ENV => Some("https://proxy.example/typesafe/v1/systemone".to_string()),
            MODEL_ENV => Some("jev-1.12.0".to_string()),
            _ => None,
        })
        .expect("overrides");
        assert_eq!(
            overridden.url,
            "https://proxy.example/typesafe/v1/systemone"
        );
        assert_eq!(overridden.model, "jev-1.12.0");

        // The primary name wins over the alias; blanks are ignored.
        let both = JevClient::from_lookup(|name| match name {
            API_KEY_ENV => Some("  ".to_string()),
            API_KEY_ALIAS_ENV => Some("alias".to_string()),
            API_URL_ENV => Some("   ".to_string()),
            _ => None,
        })
        .expect("alias after blank");
        assert_eq!(both.key, "alias");
        assert_eq!(both.url, DEFAULT_API_URL);
    }

    #[test]
    fn from_env_names_the_key_variables_when_missing() {
        let error = JevClient::from_lookup(|_| None).expect_err("missing key");
        let message = error.to_string();
        assert!(message.contains(API_KEY_ENV), "{message}");
        assert!(message.contains(API_KEY_ALIAS_ENV), "{message}");
    }

    #[test]
    fn debug_redacts_the_key() {
        let client = JevClient::new("super-secret-key", "https://api.typesafe.ai/v1/systemone");
        let rendered = format!("{client:?}");
        assert!(!rendered.contains("super-secret-key"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn rank_short_circuits_an_empty_candidate_set() {
        // No server behind this URL: reaching the network at all errors.
        let client = JevClient::new("key", "");
        let decisions = client.rank("query", &[]).expect("no call");
        assert!(decisions.is_empty());
    }

    #[test]
    fn rank_sends_system_one_schema_and_maps_probabilities() {
        let (url, received) = serve_once(
            "200 OK",
            answers(&[("candidate_1", 0.96), ("candidate_2", 0.31)]),
        );
        let client = JevClient::new("test-key", url);
        let candidates = vec![
            candidate(
                "session-a#1",
                "Changed Redis maxConnections from 20 to 100.",
            ),
            candidate("session-b#4", "Updated the Redis Docker image."),
        ];
        let decisions = client
            .rank("Where did we fix the Redis timeout?", &candidates)
            .expect("rank");
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].id, "session-a#1");
        assert_eq!(decisions[0].relevance, 0.96);
        assert_eq!(decisions[0].decision, JevDecisionKind::Retrieve);
        assert_eq!(decisions[1].decision, JevDecisionKind::Ignore);

        let request = received.recv().expect("captured request");
        let lower = request.to_lowercase();
        assert!(lower.contains("post /v1/systemone"), "{lower}");
        assert!(
            lower.contains("authorization: bearer test-key"),
            "the key must ride the auth header: {lower}"
        );
        let body = request.split("\r\n\r\n").nth(1).expect("request body");
        let json: serde_json::Value = serde_json::from_str(body).expect("request JSON");
        assert_eq!(json["model"], DEFAULT_MODEL);
        let state = json["state"].as_str().expect("state string");
        assert!(
            state.contains("Where did we fix the Redis timeout?"),
            "{state}"
        );
        assert!(state.contains("[1] session-a#1"), "{state}");
        assert!(state.contains("[2] session-b#4"), "{state}");
        assert!(state.contains("Changed Redis maxConnections"), "{state}");
        let questions = json["questions"].as_object().expect("questions object");
        assert_eq!(questions.len(), 2);
        assert_eq!(questions["candidate_1"]["type"], "noul");
        assert!(
            questions["candidate_1"]["instructions"]
                .as_str()
                .expect("instructions")
                .contains("Candidate [1] (session-a#1)"),
            "{:?}",
            questions["candidate_1"]
        );
        assert!(questions["candidate_1"]["criteria"]["true"].is_string());
        assert!(questions["candidate_2"]["criteria"]["false"].is_string());
    }

    #[test]
    fn the_threshold_decides_retrieve_versus_ignore() {
        let (url, _) = serve_once(
            "200 OK",
            answers(&[("candidate_1", 0.5), ("candidate_2", 0.4999)]),
        );
        let client = JevClient::new("key", url);
        let decisions = client
            .rank("task", &[candidate("a#1", "x"), candidate("b#1", "y")])
            .expect("rank");
        assert_eq!(decisions[0].decision, JevDecisionKind::Retrieve);
        assert_eq!(decisions[0].relevance, RETRIEVE_THRESHOLD);
        assert_eq!(decisions[1].decision, JevDecisionKind::Ignore);
    }

    #[test]
    fn incomplete_or_unexpected_answers_are_rejected() {
        let (url, _) = serve_once("200 OK", answers(&[("candidate_1", 0.9)]));
        let client = JevClient::new("key", url);
        let error = client
            .rank("task", &[candidate("a#1", "x"), candidate("b#1", "y")])
            .expect_err("candidate_2 unanswered");
        match error {
            Error::Remote { harness, detail } => {
                assert_eq!(harness, "jev");
                assert!(detail.contains("candidate_2"), "{detail}");
            }
            other => panic!("expected Remote, got {other:?}"),
        }

        let (url, _) = serve_once(
            "200 OK",
            answers(&[("candidate_1", 0.9), ("candidate_7", 0.1)]),
        );
        let client = JevClient::new("key", url);
        let error = client
            .rank("task", &[candidate("a#1", "x")])
            .expect_err("candidate_7 unknown");
        match error {
            Error::Remote { detail, .. } => {
                assert!(detail.contains("unknown question"), "{detail}");
                assert!(detail.contains("candidate_7"), "{detail}");
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn a_non_noul_answer_is_rejected() {
        let body = serde_json::json!({
            "answers": {"candidate_1": {"type": "choice", "choice": "yes"}},
        })
        .to_string();
        let (url, _) = serve_once("200 OK", body);
        let client = JevClient::new("key", url);
        let error = client
            .rank("task", &[candidate("a#1", "x")])
            .expect_err("choice for a noul question");
        match error {
            Error::Remote { detail, .. } => {
                assert!(detail.contains("noul"), "{detail}");
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn rank_maps_http_failures_to_remote_errors() {
        let (url, _) = serve_once("401 Unauthorized", "{}".to_string());
        let client = JevClient::new("bad-key", url);
        let error = client
            .rank("query", &[candidate("s#1", "x")])
            .expect_err("401");
        match error {
            Error::Remote { harness, detail } => {
                assert_eq!(harness, "jev");
                assert!(detail.contains("401"), "{detail}");
                assert!(detail.contains(API_KEY_ENV), "{detail}");
            }
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn rank_rejects_malformed_and_oversized_bodies() {
        let (url, _) = serve_once("200 OK", "not json at all".to_string());
        let client = JevClient::new("key", url);
        let error = client
            .rank("query", &[candidate("s#1", "x")])
            .expect_err("malformed");
        match error {
            Error::Remote { detail, .. } => assert!(detail.contains("JSON"), "{detail}"),
            other => panic!("expected Remote, got {other:?}"),
        }

        let oversize = "x".repeat(MAX_RESPONSE_BYTES + 1);
        let (url, _) = serve_once("200 OK", oversize);
        let client = JevClient::new("key", url);
        let error = client
            .rank("query", &[candidate("s#1", "x")])
            .expect_err("oversized");
        match error {
            Error::Remote { detail, .. } => assert!(detail.contains("exceeded"), "{detail}"),
            other => panic!("expected Remote, got {other:?}"),
        }
    }

    #[test]
    fn excerpt_truncates_on_a_character_boundary() {
        let short = excerpt("short");
        assert_eq!(short, "short");
        let long: String = "é".repeat(CANDIDATE_CONTENT_CHARS + 50);
        let cut = excerpt(&long);
        assert!(cut.ends_with("\n[truncated]"), "must mark the cut");
        // Exactly CANDIDATE_CONTENT_CHARS whole characters survive — the
        // cut lands between 'é's, never inside one.
        let kept = cut.trim_end_matches("\n[truncated]");
        assert_eq!(kept.chars().count(), CANDIDATE_CONTENT_CHARS);
        assert!(kept.ends_with('é'));
    }
}
