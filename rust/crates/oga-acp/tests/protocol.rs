//! The ACP client against a scripted agent: no account, no model, no network.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use oga_acp::{
    AcpConfig, AcpError, AcpPolicy, AcpSession, AgentRelease, Decision, DenyAll, Grants, Launch,
    PolicyFuture, Refusal, SessionSetting, SessionStart, Stage,
    schema::{
        ContentBlock, Error, PermissionOptionId, ReadTextFileRequest, RequestPermissionRequest,
        SessionId, SessionUpdate, StopReason,
    },
};
use oga_domain::Provider;
use oga_runner::{ProviderRunner, RunRequest};
use tempfile::TempDir;

fn agent(mode: &str, cwd: &Path) -> RunRequest {
    RunRequest::new(
        Provider::Claude,
        vec![
            std::env::var("CARGO_BIN_EXE_fake-agent").expect("fake agent binary"),
            mode.into(),
        ],
        cwd,
    )
}

fn config() -> AcpConfig {
    AcpConfig {
        handshake_timeout: Duration::from_secs(5),
        initialize_timeout: Duration::from_secs(5),
        prompt_timeout: Duration::from_secs(10),
        ..AcpConfig::default()
    }
}

async fn open_with<P: AcpPolicy>(
    mode: &str,
    cwd: &Path,
    policy: Arc<P>,
    config: AcpConfig,
) -> Result<AcpSession, AcpError> {
    AcpSession::open(
        &ProviderRunner::default(),
        Launch::new(agent(mode, cwd), Default::default(), SessionStart::New),
        policy,
        config,
    )
    .await
}

async fn open(mode: &str, cwd: &Path) -> Result<AcpSession, AcpError> {
    open_with(mode, cwd, Arc::new(DenyAll), config()).await
}

fn ask(text: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::from(text)]
}

#[tokio::test]
async fn a_full_turn_streams_before_it_stops() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open("turn", temp.path()).await.expect("session opened");

    assert_eq!(session.session_id(), &SessionId::from("fake-acp-session"));
    assert_eq!(
        session.agent_info().map(|info| info.name.as_str()),
        Some("fake-agent")
    );

    let mut updates = session.take_updates().expect("update stream");
    let answer = session.prompt(ask("hi")).await.expect("prompt answered");
    assert_eq!(answer.stop_reason, StopReason::EndTurn);

    let mut text = String::new();
    while let Ok(Some(update)) =
        tokio::time::timeout(Duration::from_millis(200), updates.recv()).await
    {
        text.push_str(&chunk_text(&update.update));
    }
    assert_eq!(text, "hello world");

    session.shutdown().await;
}

#[tokio::test]
async fn cancelling_ends_the_turn_without_killing_the_agent() {
    let temp = TempDir::new().expect("temporary directory");
    let session = Arc::new(open("cancel", temp.path()).await.expect("session opened"));

    let turn = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.prompt(ask("work")).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    session.cancel();

    let answer = tokio::time::timeout(Duration::from_secs(5), turn)
        .await
        .expect("the cancelled turn answers")
        .expect("the turn task finished")
        .expect("the agent answered the cancelled prompt");
    assert_eq!(answer.stop_reason, StopReason::Cancelled);

    session.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_exits_mid_prompt_leaves_the_turn_in_flight() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open("exit-after-prompt", temp.path())
        .await
        .expect("session opened");

    let error = session
        .prompt(ask("hi"))
        .await
        .expect_err("an agent that exits cannot answer");
    assert!(
        matches!(error, AcpError::PromptInFlight { .. }),
        "a sent prompt must never look retryable: {error}"
    );
    assert!(!error.allows_retry_elsewhere());
}

#[tokio::test]
async fn an_agent_error_ends_the_turn_but_keeps_the_session() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open("error-once", temp.path())
        .await
        .expect("session opened");

    let error = session
        .prompt(ask("hi"))
        .await
        .expect_err("the agent answers the first prompt with an error");
    assert!(
        matches!(error, AcpError::TurnFailed { .. }),
        "an error the agent answered with leaves its session open: {error}"
    );
    assert!(!error.allows_retry_elsewhere());

    let answer = session
        .prompt(ask("continue"))
        .await
        .expect("the same session takes the next prompt");
    assert_eq!(answer.stop_reason, StopReason::EndTurn);

    session.shutdown().await;
}

#[tokio::test]
async fn malformed_and_oversized_frames_are_dropped_not_fatal() {
    let temp = TempDir::new().expect("temporary directory");
    let mut env = BTreeMap::new();
    env.insert("FAKE_AGENT_OVERSIZE_BYTES".to_owned(), "16384".to_owned());

    let session = AcpSession::open(
        &ProviderRunner::default(),
        Launch::new(
            agent("malformed", temp.path()).with_env(env),
            Default::default(),
            SessionStart::New,
        ),
        Arc::new(DenyAll),
        AcpConfig {
            max_frame_bytes: 4 * 1024,
            ..config()
        },
    )
    .await
    .expect("session opened");

    let answer = session.prompt(ask("hi")).await.expect("prompt answered");
    assert_eq!(answer.stop_reason, StopReason::EndTurn);

    let diagnostics = session.diagnostics();
    assert_eq!(diagnostics.malformed_frames, 1);
    assert_eq!(diagnostics.oversized_frames, 1);
    assert!(diagnostics.dropped_bytes > 16_384);

    session.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_never_starts_is_unavailable() {
    let temp = TempDir::new().expect("temporary directory");
    let error = open("exit-immediately", temp.path())
        .await
        .expect_err("a dead agent cannot shake hands");
    assert!(
        matches!(
            error,
            AcpError::Unavailable {
                stage: Stage::Initialize,
                ..
            }
        ),
        "unexpected failure: {error}"
    );
    assert!(error.allows_retry_elsewhere());
}

#[tokio::test]
async fn a_silent_agent_times_out_rather_than_hanging() {
    let temp = TempDir::new().expect("temporary directory");
    let error = open_with(
        "silent",
        temp.path(),
        Arc::new(DenyAll),
        AcpConfig {
            initialize_timeout: Duration::from_millis(200),
            ..config()
        },
    )
    .await
    .expect_err("a silent agent cannot shake hands");
    assert!(
        error.allows_retry_elsewhere(),
        "unexpected failure: {error}"
    );
    assert!(
        error.to_string().contains("wrote nothing"),
        "the failure should say the agent went quiet: {error}"
    );
}

#[tokio::test]
async fn a_failed_handshake_carries_what_the_agent_wrote() {
    let temp = TempDir::new().expect("temporary directory");
    let error = open_with("exit-immediately", temp.path(), Arc::new(DenyAll), config())
        .await
        .expect_err("an agent that exits cannot shake hands");
    assert!(
        error.to_string().contains("fake agent could not start"),
        "the failure should carry the agent's own words: {error}"
    );
}

#[tokio::test]
async fn an_incompatible_protocol_is_unavailable() {
    let temp = TempDir::new().expect("temporary directory");
    let error = open("old-protocol", temp.path())
        .await
        .expect_err("a version this client cannot speak");
    assert!(
        error.allows_retry_elsewhere(),
        "unexpected failure: {error}"
    );
    assert!(error.to_string().contains("ACP 0"), "{error}");
}

#[tokio::test]
async fn an_agent_demanding_sign_in_is_never_retried_elsewhere() {
    let temp = TempDir::new().expect("temporary directory");
    let error = open("auth", temp.path())
        .await
        .expect_err("the agent wants credentials");
    assert!(
        matches!(
            error,
            AcpError::Refused {
                kind: Refusal::Authentication,
                ..
            }
        ),
        "unexpected failure: {error}"
    );
    assert!(
        !error.allows_retry_elsewhere(),
        "a refusal must not be worked around"
    );
}

#[tokio::test]
async fn permission_requests_reach_the_policy() {
    struct Allow;
    impl AcpPolicy for Allow {
        fn permission(&self, request: RequestPermissionRequest) -> PolicyFuture<'_, Decision> {
            assert_eq!(request.options.len(), 2);
            Box::pin(async { Decision::Select(PermissionOptionId::from("allow")) })
        }
    }

    let temp = TempDir::new().expect("temporary directory");
    let session = open_with("permission", temp.path(), Arc::new(Allow), config())
        .await
        .expect("session opened");
    let mut updates = session.take_updates().expect("update stream");
    session.prompt(ask("hi")).await.expect("prompt answered");

    let update = updates.recv().await.expect("the agent reported the answer");
    assert!(
        chunk_text(&update.update).contains("\"optionId\":\"allow\""),
        "{:?}",
        update.update
    );

    session.shutdown().await;
}

#[tokio::test]
async fn an_ungranted_file_read_is_refused() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open("read-file", temp.path())
        .await
        .expect("session opened");
    let mut updates = session.take_updates().expect("update stream");
    session.prompt(ask("hi")).await.expect("prompt answered");

    let update = updates.recv().await.expect("the agent reported the answer");
    assert!(
        chunk_text(&update.update).contains("error -32601"),
        "an ungranted read must be refused: {:?}",
        update.update
    );

    session.shutdown().await;
}

#[tokio::test]
async fn a_granted_file_read_reaches_the_policy() {
    struct ReadOnly;
    impl AcpPolicy for ReadOnly {
        fn grants(&self) -> Grants {
            Grants::default().read_text_file(true)
        }

        fn read_text_file(
            &self,
            request: ReadTextFileRequest,
        ) -> PolicyFuture<'_, Result<String, Error>> {
            assert_eq!(request.path, Path::new("/tmp/fake.txt"));
            Box::pin(async { Ok("file body".to_owned()) })
        }
    }

    let temp = TempDir::new().expect("temporary directory");
    let session = open_with("read-file", temp.path(), Arc::new(ReadOnly), config())
        .await
        .expect("session opened");
    let mut updates = session.take_updates().expect("update stream");
    session.prompt(ask("hi")).await.expect("prompt answered");

    let update = updates.recv().await.expect("the agent reported the answer");
    assert!(
        chunk_text(&update.update).contains("file body"),
        "{update:?}"
    );

    session.shutdown().await;
}

#[tokio::test]
async fn an_ungranted_terminal_is_refused() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open("terminal", temp.path()).await.expect("session opened");
    let mut updates = session.take_updates().expect("update stream");
    session.prompt(ask("hi")).await.expect("prompt answered");

    let update = updates.recv().await.expect("the agent reported the answer");
    assert!(
        chunk_text(&update.update).contains("error -32601"),
        "an ungranted terminal must be refused: {:?}",
        update.update
    );

    session.shutdown().await;
}

#[tokio::test]
async fn a_loaded_session_keeps_the_agents_own_id() {
    let temp = TempDir::new().expect("temporary directory");
    let session = AcpSession::open(
        &ProviderRunner::default(),
        Launch::new(
            agent("turn", temp.path()),
            Default::default(),
            SessionStart::Load(SessionId::from("earlier-session")),
        ),
        Arc::new(DenyAll),
        config(),
    )
    .await
    .expect("session loaded");

    assert_eq!(session.session_id(), &SessionId::from("earlier-session"));
    session.shutdown().await;
}

async fn open_release(release: AgentRelease, cwd: &Path) -> Result<AcpSession, AcpError> {
    AcpSession::open(
        &ProviderRunner::default(),
        Launch::new(agent("turn", cwd), Default::default(), SessionStart::New).release(release),
        Arc::new(DenyAll),
        config(),
    )
    .await
}

#[tokio::test]
async fn only_the_release_a_launch_was_verified_against_opens_a_session() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open_release(AgentRelease::line("fake-agent", "1.0"), temp.path())
        .await
        .expect("a patch release of the verified line");
    session.shutdown().await;

    for release in [
        AgentRelease::line("fake-agent", "1.1"),
        AgentRelease::line("fake-agent", "1"),
        AgentRelease::line("another-agent", "1.0"),
    ] {
        let error = open_release(release, temp.path())
            .await
            .expect_err("an agent the launch was not verified against");
        assert!(
            matches!(
                error,
                AcpError::Unavailable {
                    stage: Stage::Initialize,
                    ..
                }
            ),
            "turned away before any session opens: {error}"
        );
        assert!(
            error.to_string().contains("not fake-agent 1.0.0"),
            "{error}"
        );
    }
}

async fn open_configured(
    mode: &str,
    cwd: &Path,
    settings: Vec<SessionSetting>,
) -> Result<AcpSession, AcpError> {
    AcpSession::open(
        &ProviderRunner::default(),
        Launch::new(agent(mode, cwd), Default::default(), SessionStart::New).settings(settings),
        Arc::new(DenyAll),
        config(),
    )
    .await
}

/// The text of one turn, which a configurable agent spends reporting the
/// settings it ended up holding.
async fn answer(session: &AcpSession) -> String {
    let mut updates = session.take_updates().expect("update stream");
    session.prompt(ask("hi")).await.expect("prompt answered");
    let mut text = String::new();
    while let Ok(Some(update)) =
        tokio::time::timeout(Duration::from_millis(200), updates.recv()).await
    {
        text.push_str(&chunk_text(&update.update));
    }
    text
}

#[tokio::test]
async fn settings_are_chosen_in_order_from_what_the_agent_offers() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open_configured(
        "configurable",
        temp.path(),
        vec![
            SessionSetting::required("model", "deep"),
            SessionSetting::required("effort", "high"),
        ],
    )
    .await
    .expect("session opened");

    assert_eq!(answer(&session).await, "model=deep effort=high changes=2");
    session.shutdown().await;
}

#[tokio::test]
async fn a_setting_already_held_or_not_required_sends_nothing() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open_configured(
        "configurable",
        temp.path(),
        vec![
            SessionSetting::required("model", "fast"),
            SessionSetting::if_offered("effort", "high"),
        ],
    )
    .await
    .expect("session opened");

    assert_eq!(
        answer(&session).await,
        "model=fast effort=default changes=0"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn a_required_choice_the_agent_does_not_offer_is_unavailable_before_any_prompt() {
    let temp = TempDir::new().expect("temporary directory");
    for (mode, setting) in [
        ("turn", SessionSetting::required("model", "fast")),
        ("configurable", SessionSetting::required("model", "huge")),
        ("configurable", SessionSetting::required("effort", "high")),
    ] {
        let error = open_configured(mode, temp.path(), vec![setting.clone()])
            .await
            .expect_err("an unoffered choice is never guessed at");
        assert!(
            matches!(
                error,
                AcpError::Unavailable {
                    stage: Stage::Configure,
                    ..
                }
            ),
            "{mode} {setting:?}: {error}"
        );
        assert!(error.allows_retry_elsewhere());
    }
}

#[tokio::test]
async fn stderr_is_kept_for_diagnosis() {
    let temp = TempDir::new().expect("temporary directory");
    let session = open("noisy", temp.path()).await.expect("session opened");
    session.prompt(ask("hi")).await.expect("prompt answered");

    let diagnostics = session.diagnostics();
    assert!(
        diagnostics.stderr.contains("deprecated flag"),
        "stderr was not kept: {diagnostics:?}"
    );
    assert!(!diagnostics.stderr_truncated);

    session.shutdown().await;
}

fn chunk_text(update: &SessionUpdate) -> String {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
            ContentBlock::Text(content) => content.text.clone(),
            other => panic!("unexpected content: {other:?}"),
        },
        other => panic!("unexpected update: {other:?}"),
    }
}
