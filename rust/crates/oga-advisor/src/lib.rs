//! Asks TypeSafe's Jev which connected worker should run a task, reading the
//! brief the caller already typed and nothing else.
//!
//! The answer is advice. Every outcome short of a confident pick of one of
//! the destinations offered — no key, a refused call, a slow one, an answer
//! naming something that was never on the table — comes back as nothing, and
//! the routing rules decide as they always have.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

/// Where the question goes unless a caller points it somewhere else.
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// The flagship build. A pinned one (`jev-1.13.0`) answers the same shape.
const MODEL: &str = "jev-latest";

/// Below this the distribution is too flat to act on: the rules already hold
/// an answer, and a near-coin-flip is not a better one.
const MIN_CONFIDENCE: f64 = 0.5;

/// Long enough for one small call on a slow connection, short enough that a
/// stalled advisor costs a dispatch a few seconds instead of holding it.
const TIMEOUT: Duration = Duration::from_secs(4);

/// The question's key, in the request and in the answer.
const QUESTION: &str = "worker";

const INSTRUCTIONS: &str = "Pick the worker that should run this task. The state is the brief \
     the task will be given. Weigh what the brief actually asks for against what each worker is \
     good at.";

/// One destination the advisor may pick, as it is described to the advisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub profile_id: String,
    pub model: String,
    /// What this destination is good at, in the advisor's own terms.
    pub description: String,
}

impl Destination {
    /// The name this destination answers to in the criteria and the answer.
    fn key(&self) -> String {
        format!("{}:{}", self.profile_id, self.model)
    }
}

/// Where the advisor would send this task, and how concentrated the
/// distribution behind that was.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub profile_id: String,
    pub model: String,
    pub confidence: f64,
}

impl Choice {
    /// Whether the distribution is concentrated enough to route on.
    pub fn confident(&self) -> bool {
        self.confidence >= MIN_CONFIDENCE
    }
}

/// A signed-in advisor. Holds the key, so it is never printed: no `Debug`,
/// and nothing here writes the key anywhere but the request header.
pub struct Advisor {
    endpoint: String,
    api_key: String,
}

impl Advisor {
    pub fn new(api_key: impl Into<String>) -> Self {
        Advisor {
            endpoint: ENDPOINT.to_owned(),
            api_key: api_key.into(),
        }
    }

    /// Send the question to a stub instead of TypeSafe, so no test reaches
    /// the real endpoint.
    #[cfg(test)]
    fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Ask where `state` should run. `None` covers every failure the caller
    /// treats alike: fewer than two destinations to choose between, a
    /// transport or status failure, a body that does not parse, and an answer
    /// naming something outside `destinations`. A pick the distribution does
    /// not back still comes back, so the caller can record what was advised
    /// before falling back from it — see [`Choice::confident`].
    pub async fn choose(&self, state: &str, destinations: &[Destination]) -> Option<Choice> {
        if destinations.len() < 2 {
            return None;
        }
        let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok()?;
        let response = client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request_body(state, destinations))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        read_choice(&response.json::<Value>().await.ok()?, destinations)
    }
}

/// The body TypeSafe reads: the brief as the state, and one Choice question
/// whose criteria are the destinations and what each is good at.
fn request_body(state: &str, destinations: &[Destination]) -> Value {
    let criteria: serde_json::Map<String, Value> = destinations
        .iter()
        .map(|destination| (destination.key(), json!(destination.description)))
        .collect();
    json!({
        "state": state,
        "model": MODEL,
        "questions": {
            QUESTION: {
                "type": "choice",
                "instructions": INSTRUCTIONS,
                "criteria": criteria,
            }
        }
    })
}

#[derive(Deserialize)]
struct ChoiceAnswer {
    choice: String,
    confidence: f64,
}

/// Reads an answer back to the destination it names. A missing answer, a key
/// no destination carries, or a body of another shape all read as no advice.
fn read_choice(body: &Value, destinations: &[Destination]) -> Option<Choice> {
    let answer: ChoiceAnswer =
        serde_json::from_value(body.get("answers")?.get(QUESTION)?.clone()).ok()?;
    let picked = destinations
        .iter()
        .find(|destination| destination.key() == answer.choice)?;
    Some(Choice {
        profile_id: picked.profile_id.clone(),
        model: picked.model.clone(),
        confidence: answer.confidence,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    use super::*;

    fn destinations() -> Vec<Destination> {
        vec![
            Destination {
                profile_id: "fast".into(),
                model: "vendor/small".into(),
                description: "free".into(),
            },
            Destination {
                profile_id: "deep".into(),
                model: "vendor/large".into(),
                description: "reasoning, long-context".into(),
            },
        ]
    }

    /// Serves one canned response on a loopback port and hands back its URL,
    /// so the failure paths are exercised without reaching TypeSafe.
    fn stub(status: &str, payload: &str) -> String {
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = drain_request(&mut stream);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{address}/v1/systemone")
    }

    fn drain_request(stream: &mut TcpStream) -> std::io::Result<()> {
        use std::io::Read;
        let mut buffer = [0u8; 8192];
        stream.read(&mut buffer).map(|_| ())
    }

    #[test]
    fn the_request_carries_the_brief_the_model_and_one_choice_question() {
        let body = request_body("port the parser", &destinations());

        assert_eq!(body["state"], "port the parser");
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["questions"]["worker"]["type"], "choice");
        let criteria = &body["questions"]["worker"]["criteria"];
        assert_eq!(criteria["fast:vendor/small"], "free");
        assert_eq!(criteria["deep:vendor/large"], "reasoning, long-context");
    }

    #[test]
    fn an_answer_reads_back_to_the_destination_it_names() {
        let answered = json!({
            "model": "jev-1.13.0",
            "answers": {
                "worker": {
                    "type": "choice",
                    "choice": "deep:vendor/large",
                    "probabilities": { "deep:vendor/large": 0.87, "fast:vendor/small": 0.13 },
                    "confidence": 0.87,
                }
            },
        });

        let choice = read_choice(&answered, &destinations()).expect("choice");

        assert_eq!(choice.profile_id, "deep");
        assert_eq!(choice.model, "vendor/large");
        assert!(choice.confident());
    }

    #[test]
    fn a_thin_answer_still_reads_so_it_can_be_recorded_before_the_fallback() {
        let answered = json!({
            "answers": { "worker": { "choice": "fast:vendor/small", "confidence": 0.31 } },
        });

        let choice = read_choice(&answered, &destinations()).expect("choice");

        assert_eq!(choice.confidence, 0.31);
        assert!(!choice.confident());
    }

    #[test]
    fn an_answer_naming_something_that_was_never_offered_is_no_advice() {
        let answered = json!({
            "answers": { "worker": { "choice": "other:vendor/huge", "confidence": 0.99 } },
        });

        assert_eq!(read_choice(&answered, &destinations()), None);
    }

    #[test]
    fn a_body_of_another_shape_is_no_advice() {
        assert_eq!(read_choice(&json!({}), &destinations()), None);
        assert_eq!(
            read_choice(&json!({ "answers": { "worker": {} } }), &destinations()),
            None
        );
        assert_eq!(
            read_choice(
                &json!({ "answers": { "worker": { "type": "noul", "noul": 0.9 } } }),
                &destinations()
            ),
            None
        );
    }

    #[tokio::test]
    async fn one_destination_is_not_a_choice_worth_asking_about() {
        let advisor = Advisor::new("key").endpoint("http://127.0.0.1:1/v1/systemone");

        assert_eq!(advisor.choose("anything", &destinations()[..1]).await, None);
    }

    #[tokio::test]
    async fn a_refused_key_comes_back_as_no_advice() {
        let advisor = Advisor::new("key")
            .endpoint(stub("401 Unauthorized", r#"{"error":"invalid api key"}"#));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            None
        );
    }

    #[tokio::test]
    async fn a_rejected_request_comes_back_as_no_advice() {
        let advisor = Advisor::new("key").endpoint(stub(
            "422 Unprocessable Entity",
            r#"{"error":"bad criteria"}"#,
        ));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            None
        );
    }

    #[tokio::test]
    async fn a_rate_limit_comes_back_as_no_advice() {
        let advisor =
            Advisor::new("key").endpoint(stub("429 Too Many Requests", r#"{"error":"slow down"}"#));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            None
        );
    }

    #[tokio::test]
    async fn an_overloaded_advisor_comes_back_as_no_advice() {
        let advisor =
            Advisor::new("key").endpoint(stub("529 Overloaded", r#"{"error":"overloaded"}"#));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            None
        );
    }

    #[tokio::test]
    async fn an_unreachable_advisor_comes_back_as_no_advice() {
        let advisor = Advisor::new("key").endpoint("http://127.0.0.1:1/v1/systemone");

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            None
        );
    }

    #[tokio::test]
    async fn a_malformed_body_comes_back_as_no_advice() {
        let advisor = Advisor::new("key").endpoint(stub("200 OK", "not json at all"));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            None
        );
    }

    #[tokio::test]
    async fn a_confident_answer_names_the_destination_it_picked() {
        let advisor = Advisor::new("key").endpoint(stub(
            "200 OK",
            r#"{"model":"jev-1.13.0","answers":{"worker":{"type":"choice","choice":"deep:vendor/large","probabilities":{"deep:vendor/large":0.9,"fast:vendor/small":0.1},"confidence":0.9}},"usage":{"input_tokens":1,"output_tokens":1}}"#,
        ));

        let choice = advisor
            .choose("port the parser", &destinations())
            .await
            .expect("choice");

        assert_eq!(choice.model, "vendor/large");
        assert!(choice.confident());
    }
}
