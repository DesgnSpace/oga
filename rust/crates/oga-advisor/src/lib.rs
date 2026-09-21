//! Asks TypeSafe's Jev which connected worker should run a task, reading the
//! brief the caller already typed and nothing else.
//!
//! Its pick decides where the task runs. Every outcome short of a pick of
//! one of the destinations offered — a refused call, a slow one, an answer
//! naming something that was never on the table — comes back as the reason
//! there was no pick, and the routing rules decide as they always have.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

/// Where the question goes unless a caller points it somewhere else.
const ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// The flagship build. A pinned one (`jev-1.13.0`) answers the same shape.
const MODEL: &str = "jev-latest";

/// Long enough for a long brief weighed against every enabled model, which
/// takes a few seconds on its own; short enough that a stalled advisor delays
/// a dispatch instead of holding it.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The worker question's key, in the request and in the answer.
const QUESTION: &str = "worker";

const INSTRUCTIONS: &str = "Pick the worker that should run this task. The state is the brief \
     the task will be given. Weigh what the brief actually asks for against what each worker is \
     good at. Among workers that can do the job well, prefer a free or cheaper one, and one whose \
     unused allowance resets soon, so it is spent rather than lost; avoid one close to its limit \
     or out of credits. Never trade away a worker the job needs to save money.";

/// The effort question's key, in the request and in the answer.
const EFFORT_QUESTION: &str = "effort";

const EFFORT_INSTRUCTIONS: &str = "Pick how hard the worker should think on this task. The state \
     is the brief the task will be given. More effort is slower and costs more; spend it only \
     where the brief needs careful reasoning.";

/// Every effort level a worker may accept, weakest first, with what it is
/// worth spending on.
const EFFORTS: [(&str, &str); 6] = [
    ("minimal", "a mechanical edit with nothing to decide"),
    ("low", "a small, clear change or a quick lookup"),
    ("medium", "ordinary feature work across a few files"),
    ("high", "work that needs a plan, or care across many files"),
    (
        "xhigh",
        "hard reasoning: races, security, subtle correctness",
    ),
    (
        "max",
        "the hardest problems, where a wrong answer is costly",
    ),
];

/// One destination the advisor may pick, as it is described to the advisor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub profile_id: String,
    pub model: String,
    /// What this destination is good at, in the advisor's own terms.
    pub description: String,
    /// Effort levels this destination accepts. Empty when none is published.
    pub efforts: Vec<String>,
}

impl Destination {
    /// The name this destination answers to in the criteria and the answer.
    fn key(&self) -> String {
        format!("{}:{}", self.profile_id, self.model)
    }
}

/// Where the advisor would send this task, and how much of its probability
/// went to that pick.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub profile_id: String,
    pub model: String,
    pub confidence: f64,
    /// How hard the picked worker should think, when the advisor answered
    /// with a level that worker accepts.
    pub effort: Option<String>,
}

/// Why the advisor gave no pick. Its `Display` is the reason recorded on the
/// task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoAdvice {
    /// Fewer than two destinations, so there was nothing to choose between.
    NothingToChoose,
    TimedOut,
    Unreachable,
    /// TypeSafe answered with this HTTP status.
    Refused(u16),
    /// The answer was not the shape a pick comes back in.
    Unreadable,
    /// The answer named a worker that was never offered.
    NotOffered,
}

impl std::fmt::Display for NoAdvice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoAdvice::NothingToChoose => write!(f, "only one worker could take this task"),
            NoAdvice::TimedOut => write!(f, "TypeSafe took longer than {}s", TIMEOUT.as_secs()),
            NoAdvice::Unreachable => write!(f, "TypeSafe could not be reached"),
            NoAdvice::Refused(401 | 403) => write!(f, "TypeSafe did not accept the key"),
            NoAdvice::Refused(429) => write!(f, "TypeSafe asked to slow down"),
            NoAdvice::Refused(status) => write!(f, "TypeSafe answered with error {status}"),
            NoAdvice::Unreadable => write!(f, "TypeSafe's answer could not be read"),
            NoAdvice::NotOffered => write!(f, "TypeSafe picked a worker that was not offered"),
        }
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

    /// Ask where `state` should run, or say why there is no pick.
    pub async fn choose(
        &self,
        state: &str,
        destinations: &[Destination],
    ) -> Result<Choice, NoAdvice> {
        if destinations.len() < 2 {
            return Err(NoAdvice::NothingToChoose);
        }
        let sent = |error: reqwest::Error| {
            if error.is_timeout() {
                NoAdvice::TimedOut
            } else {
                NoAdvice::Unreachable
            }
        };
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| NoAdvice::Unreachable)?;
        let response = client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request_body(state, destinations))
            .send()
            .await
            .map_err(sent)?;
        if !response.status().is_success() {
            return Err(NoAdvice::Refused(response.status().as_u16()));
        }
        let body = response.json::<Value>().await.map_err(|error| {
            if error.is_timeout() {
                NoAdvice::TimedOut
            } else {
                NoAdvice::Unreadable
            }
        })?;
        read_choice(&body, destinations)
    }
}

/// The body TypeSafe reads: the brief as the state, a Choice question whose
/// criteria are the destinations and what each is good at, and a second one
/// for the effort when any destination accepts one.
fn request_body(state: &str, destinations: &[Destination]) -> Value {
    let criteria: serde_json::Map<String, Value> = destinations
        .iter()
        .map(|destination| (destination.key(), json!(destination.description)))
        .collect();
    let mut questions = serde_json::Map::new();
    questions.insert(
        QUESTION.into(),
        json!({
            "type": "choice",
            "instructions": INSTRUCTIONS,
            "criteria": criteria,
        }),
    );
    let efforts: serde_json::Map<String, Value> = EFFORTS
        .iter()
        .filter(|(level, _)| {
            destinations
                .iter()
                .any(|destination| destination.efforts.iter().any(|offered| offered == level))
        })
        .map(|(level, worth)| ((*level).to_owned(), json!(worth)))
        .collect();
    if efforts.len() >= 2 {
        questions.insert(
            EFFORT_QUESTION.into(),
            json!({
                "type": "choice",
                "instructions": EFFORT_INSTRUCTIONS,
                "criteria": efforts,
            }),
        );
    }
    json!({
        "state": state,
        "model": MODEL,
        "questions": questions,
    })
}

#[derive(Deserialize)]
struct ChoiceAnswer {
    choice: String,
    confidence: f64,
}

/// Reads an answer back to the destination it names. A missing answer, a key
/// no destination carries, or a body of another shape all read as no advice.
/// An effort answer that is missing or names a level the picked destination
/// does not accept leaves the effort to the rules.
fn read_choice(body: &Value, destinations: &[Destination]) -> Result<Choice, NoAdvice> {
    let answers = body.get("answers").ok_or(NoAdvice::Unreadable)?;
    let answer: ChoiceAnswer = answers
        .get(QUESTION)
        .and_then(|answer| serde_json::from_value(answer.clone()).ok())
        .ok_or(NoAdvice::Unreadable)?;
    let picked = destinations
        .iter()
        .find(|destination| destination.key() == answer.choice)
        .ok_or(NoAdvice::NotOffered)?;
    let effort = answers
        .get(EFFORT_QUESTION)
        .and_then(|effort| serde_json::from_value::<ChoiceAnswer>(effort.clone()).ok())
        .map(|effort| effort.choice)
        .filter(|level| picked.efforts.contains(level));
    Ok(Choice {
        profile_id: picked.profile_id.clone(),
        model: picked.model.clone(),
        confidence: answer.confidence,
        effort,
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
                efforts: vec![],
            },
            Destination {
                profile_id: "deep".into(),
                model: "vendor/large".into(),
                description: "reasoning, long-context".into(),
                efforts: vec!["low".into(), "medium".into(), "high".into()],
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
        let efforts = body["questions"]["effort"]["criteria"]
            .as_object()
            .expect("effort criteria");
        let mut levels: Vec<_> = efforts.keys().map(String::as_str).collect();
        levels.sort_unstable();
        assert_eq!(levels, ["high", "low", "medium"]);
    }

    #[test]
    fn an_effort_the_picked_worker_accepts_reads_back_with_the_choice() {
        let answered = json!({
            "answers": {
                "worker": { "choice": "deep:vendor/large", "confidence": 0.9 },
                "effort": { "choice": "high", "confidence": 0.8 },
            },
        });

        let choice = read_choice(&answered, &destinations()).expect("choice");

        assert_eq!(choice.effort.as_deref(), Some("high"));
    }

    #[test]
    fn an_effort_the_picked_worker_does_not_accept_is_left_to_the_rules() {
        let answered = json!({
            "answers": {
                "worker": { "choice": "fast:vendor/small", "confidence": 0.9 },
                "effort": { "choice": "high", "confidence": 0.8 },
            },
        });

        let choice = read_choice(&answered, &destinations()).expect("choice");

        assert_eq!(choice.effort, None);
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
    }

    #[test]
    fn an_unsure_answer_is_still_a_pick() {
        let answered = json!({
            "answers": { "worker": { "choice": "fast:vendor/small", "confidence": 0.31 } },
        });

        let choice = read_choice(&answered, &destinations()).expect("choice");

        assert_eq!(choice.model, "vendor/small");
        assert_eq!(choice.confidence, 0.31);
    }

    #[test]
    fn an_answer_naming_something_that_was_never_offered_is_no_advice() {
        let answered = json!({
            "answers": { "worker": { "choice": "other:vendor/huge", "confidence": 0.99 } },
        });

        assert_eq!(
            read_choice(&answered, &destinations()),
            Err(NoAdvice::NotOffered)
        );
    }

    #[test]
    fn a_body_of_another_shape_is_no_advice() {
        assert_eq!(
            read_choice(&json!({}), &destinations()),
            Err(NoAdvice::Unreadable)
        );
        assert_eq!(
            read_choice(&json!({ "answers": { "worker": {} } }), &destinations()),
            Err(NoAdvice::Unreadable)
        );
        assert_eq!(
            read_choice(
                &json!({ "answers": { "worker": { "type": "noul", "noul": 0.9 } } }),
                &destinations()
            ),
            Err(NoAdvice::Unreadable)
        );
    }

    #[tokio::test]
    async fn one_destination_is_not_a_choice_worth_asking_about() {
        let advisor = Advisor::new("key").endpoint("http://127.0.0.1:1/v1/systemone");

        assert_eq!(
            advisor.choose("anything", &destinations()[..1]).await,
            Err(NoAdvice::NothingToChoose)
        );
    }

    #[tokio::test]
    async fn a_refused_key_comes_back_as_no_advice() {
        let advisor = Advisor::new("key")
            .endpoint(stub("401 Unauthorized", r#"{"error":"invalid api key"}"#));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            Err(NoAdvice::Refused(401))
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
            Err(NoAdvice::Refused(422))
        );
    }

    #[tokio::test]
    async fn a_rate_limit_comes_back_as_no_advice() {
        let advisor =
            Advisor::new("key").endpoint(stub("429 Too Many Requests", r#"{"error":"slow down"}"#));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            Err(NoAdvice::Refused(429))
        );
    }

    #[tokio::test]
    async fn an_overloaded_advisor_comes_back_as_no_advice() {
        let advisor =
            Advisor::new("key").endpoint(stub("529 Overloaded", r#"{"error":"overloaded"}"#));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            Err(NoAdvice::Refused(529))
        );
    }

    #[tokio::test]
    async fn an_unreachable_advisor_comes_back_as_no_advice() {
        let advisor = Advisor::new("key").endpoint("http://127.0.0.1:1/v1/systemone");

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            Err(NoAdvice::Unreachable)
        );
    }

    #[tokio::test]
    async fn a_malformed_body_comes_back_as_no_advice() {
        let advisor = Advisor::new("key").endpoint(stub("200 OK", "not json at all"));

        assert_eq!(
            advisor.choose("port the parser", &destinations()).await,
            Err(NoAdvice::Unreadable)
        );
    }

    #[tokio::test]
    async fn an_answer_names_the_destination_it_picked() {
        let advisor = Advisor::new("key").endpoint(stub(
            "200 OK",
            r#"{"model":"jev-1.13.0","answers":{"worker":{"type":"choice","choice":"deep:vendor/large","probabilities":{"deep:vendor/large":0.9,"fast:vendor/small":0.1},"confidence":0.9}},"usage":{"input_tokens":1,"output_tokens":1}}"#,
        ));

        let choice = advisor
            .choose("port the parser", &destinations())
            .await
            .expect("choice");

        assert_eq!(choice.model, "vendor/large");
    }
}
