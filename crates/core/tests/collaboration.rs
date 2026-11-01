//! Two independent nodes over authenticated loopback QUIC, without tmux or AI.
//! These tests validate the protocol, not physical-device or AI execution.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iroh::{EndpointAddr, TransportAddr};
use p2pcli_core::client::RemoteHost;
use p2pcli_core::collaboration::delivery;
use p2pcli_core::collaboration::model::*;
use p2pcli_core::host::{BoxFuture, Decision, HostDelegate, HostEvent, IncomingRequest};
use p2pcli_core::node::{Node, NodeConfig};
use p2pcli_core::proto::Msg;
use serde_json::json;

struct Delegate;

impl HostDelegate for Delegate {
    fn approve(&self, _: IncomingRequest) -> BoxFuture<Decision> {
        Box::pin(async { Decision::Allow { trust: false } })
    }

    fn event(&self, _: HostEvent) {}
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("p2pcli-collaboration-{}", new_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn config(&self, name: &str) -> NodeConfig {
        NodeConfig {
            data_dir: self.0.join(name),
            inbox_dir: Some(self.0.join(format!("{name}-inbox"))),
            device_name: Some(name.into()),
            offline: true,
            bind_ip: Some(Ipv4Addr::LOCALHOST.into()),
            ..Default::default()
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn local_addr(node: &Node) -> EndpointAddr {
    let addresses: Vec<_> = node
        .endpoint
        .bound_sockets()
        .into_iter()
        .filter(|address| address.is_ipv4())
        .map(|address| {
            TransportAddr::Ip(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), address.port()))
        })
        .collect();
    assert!(
        !addresses.is_empty(),
        "node must expose an IPv4 loopback socket"
    );
    EndpointAddr::from_parts(node.id(), addresses)
}

async fn connect(host: &Node, guest: &Node) -> RemoteHost {
    host.host.set_password("abc234");
    tokio::time::timeout(
        Duration::from_secs(15),
        guest.connect_addr(local_addr(host), Some("abc234".into())),
    )
    .await
    .expect("loopback authentication timed out")
    .expect("loopback authentication failed")
}

fn request(project: &Project, auth: Option<&SessionAuth>, action: Action) -> Request {
    let mut request = Request::new(Some(project.id.clone()), action);
    request.auth = auth.cloned();
    request
}

fn response(reply: Reply) -> Response {
    assert!(
        reply.error.is_none(),
        "unexpected business error: {reply:?}"
    );
    reply
        .response
        .expect("successful reply must include a response")
}

fn task(reply: Reply) -> Task {
    match response(reply) {
        Response::Task { task } => task,
        other => panic!("expected task, got {other:?}"),
    }
}

fn rejected(reply: Reply, code: &str) {
    assert!(
        reply.response.is_none(),
        "rejection must not contain success: {reply:?}"
    );
    assert_eq!(reply.error.expect("expected business rejection").code, code);
}

fn registered(reply: Reply) -> (AgentSession, SessionAuth) {
    match response(reply) {
        Response::Registered { session, token } => {
            assert!(!token.is_empty());
            let auth = SessionAuth {
                session_id: session.id.clone(),
                token,
            };
            (session, auth)
        }
        other => panic!("expected session registration, got {other:?}"),
    }
}

fn register_action(name: &str) -> Action {
    Action::SessionRegister {
        name: name.into(),
        mode: SessionMode::ManualPull,
        terminal: None,
        cwd: None,
    }
}

fn assignee(session: &AgentSession) -> SessionRef {
    SessionRef {
        device_id: session.device_id.clone(),
        session_id: session.id.clone(),
    }
}

fn result(summary: &str) -> TaskResult {
    TaskResult {
        summary: summary.into(),
        completed: vec!["protocol fixture completed".into()],
        remaining: vec![],
        verification: vec!["synthetic protocol result; no AI or external command executed".into()],
        artifact_ids: vec![],
        failed: false,
    }
}

struct Pair {
    host: Arc<Node>,
    guest: Arc<Node>,
    remote: Arc<RemoteHost>,
    project: Project,
    worker: AgentSession,
    auth: SessionAuth,
    // Keep the directory alive until all node and connection handles are dropped.
    dir: TestDir,
}

impl Pair {
    async fn new() -> Self {
        let dir = TestDir::new();
        let host = Node::start(dir.config("host"), Arc::new(Delegate))
            .await
            .unwrap();
        let guest = Node::start(dir.config("guest"), Arc::new(Delegate))
            .await
            .unwrap();
        assert_ne!(
            host.id(),
            guest.id(),
            "independent data directories need distinct identities"
        );
        let remote = Arc::new(connect(&host, &guest).await);
        guest.host.paired.add(&host.id(), "host", "test").unwrap();
        guest.adopt_remote(remote.clone());
        let project = match response(
            host.collaborate(
                None,
                Request::new(
                    None,
                    Action::ProjectCreate {
                        name: "loopback collaboration".into(),
                    },
                ),
            )
            .await
            .unwrap(),
        ) {
            Response::Project { project } => project,
            other => panic!("expected project, got {other:?}"),
        };
        assert_eq!(project.coordinator_id, host.id().to_string());
        response(
            host.collaborate(
                None,
                request(
                    &project,
                    None,
                    Action::MemberSet {
                        device_id: guest.id().to_string(),
                        role: Role::Worker,
                        revoked: false,
                    },
                ),
            )
            .await
            .unwrap(),
        );
        let (worker, auth) = registered(
            remote
                .collaborate(request(&project, None, register_action("guest worker")))
                .await
                .unwrap(),
        );
        assert_eq!(worker.device_id, guest.id().to_string());
        Self {
            host,
            guest,
            remote,
            project,
            worker,
            auth,
            dir,
        }
    }

    async fn local(&self, action: Action) -> Reply {
        self.host
            .collaborate(None, request(&self.project, None, action))
            .await
            .unwrap()
    }

    async fn worker(&self, action: Action) -> Reply {
        self.remote
            .collaborate(request(&self.project, Some(&self.auth), action))
            .await
            .unwrap()
    }

    async fn create_task(&self) -> Task {
        let task = task(
            self.local(Action::TaskCreate {
                spec: TaskSpec {
                    title: "protocol test task".into(),
                    goal: "validate explicit task ownership and durable replies".into(),
                    acceptance: "worker submits a result and coordinator accepts it".into(),
                    inputs: json!({
                        "repository": "fixture://p2pcli",
                        "baseline_commit": "0123456789012345678901234567890123456789"
                    }),
                    assignee: assignee(&self.worker),
                },
            })
            .await,
        );
        assert_eq!(task.state, TaskState::Queued);
        assert!(task.attempts.is_empty());
        task
    }

    async fn get_task(&self, id: &str) -> Task {
        task(self.local(Action::TaskGet { task_id: id.into() }).await)
    }

    async fn claim(&self, queued: &Task) -> Task {
        let running = task(
            self.worker(Action::TaskClaim {
                task_id: queued.id.clone(),
                expected_version: queued.version,
            })
            .await,
        );
        assert_eq!(running.state, TaskState::Running);
        assert_eq!(running.attempts.len(), 1);
        assert_eq!(running.attempts[0].assignee, assignee(&self.worker));
        running
    }

    async fn events(&self, after: u64) -> (Vec<Event>, u64) {
        match response(
            self.local(Action::EventsRead {
                after,
                limit: MAX_EVENTS,
            })
            .await,
        ) {
            Response::Events { events, cursor } => (events, cursor),
            other => panic!("expected events, got {other:?}"),
        }
    }

    async fn shutdown(self) {
        self.remote.close();
        self.guest.shutdown().await;
        self.host.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_nodes_complete_assigned_task_and_accept_result() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    // Exercise explicit assignment as well as the assignee supplied at creation.
    let assigned = task(
        pair.local(Action::TaskAssign {
            task_id: queued.id.clone(),
            assignee: assignee(&pair.worker),
            expected_version: queued.version,
        })
        .await,
    );
    assert!(assigned.version > queued.version);
    let running = pair.claim(&assigned).await;
    let attempt_id = running.attempts[0].id.clone();
    let progress = task(
        pair.worker(Action::TaskProgress {
            task_id: running.id.clone(),
            attempt_id: attempt_id.clone(),
            message: "fixture is reporting progress".into(),
            blocked: false,
            expected_version: running.version,
        })
        .await,
    );
    assert_eq!(
        progress.progress.as_deref(),
        Some("fixture is reporting progress")
    );
    assert!(progress.version > running.version);
    let expected = result("ready for coordinator review");
    let ready = task(
        pair.worker(Action::TaskResult {
            task_id: running.id.clone(),
            attempt_id,
            result: expected.clone(),
            expected_version: progress.version,
        })
        .await,
    );
    assert_eq!(ready.state, TaskState::ResultReady);
    assert_eq!(ready.attempts[0].result.as_ref(), Some(&expected));
    rejected(
        pair.worker(Action::TaskAccept {
            task_id: ready.id.clone(),
            expected_version: ready.version,
        })
        .await,
        "forbidden",
    );
    let completed = task(
        pair.local(Action::TaskAccept {
            task_id: ready.id.clone(),
            expected_version: ready.version,
        })
        .await,
    );
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(pair.get_task(&completed.id).await, completed);
    let (events, cursor) = pair.events(0).await;
    assert!(!events.is_empty());
    assert!(
        events
            .windows(2)
            .all(|events| events[0].seq < events[1].seq)
    );
    assert!(cursor >= events.last().unwrap().seq);
    assert!(pair.events(cursor).await.0.is_empty());
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_device_is_not_automatically_a_project_member() {
    let pair = Pair::new().await;
    let other = match response(
        pair.host
            .collaborate(
                None,
                Request::new(
                    None,
                    Action::ProjectCreate {
                        name: "private project".into(),
                    },
                ),
            )
            .await
            .unwrap(),
    ) {
        Response::Project { project } => project,
        other => panic!("expected project, got {other:?}"),
    };
    for action in [Action::ProjectGet, register_action("unauthorized session")] {
        rejected(
            pair.remote
                .collaborate(request(&other, None, action))
                .await
                .unwrap(),
            "forbidden",
        );
    }
    let queued = pair.create_task().await;
    rejected(
        pair.remote
            .collaborate(request(
                &other,
                Some(&pair.auth),
                Action::TaskGet {
                    task_id: queued.id.clone(),
                },
            ))
            .await
            .unwrap(),
        "forbidden",
    );
    assert_eq!(pair.get_task(&queued.id).await, queued);
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_token_and_other_devices_valid_token_cannot_claim() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let mut forged = pair.auth.clone();
    forged.token = format!("forged-{}", new_id());
    let claim = Action::TaskClaim {
        task_id: queued.id.clone(),
        expected_version: queued.version,
    };
    rejected(
        pair.remote
            .collaborate(request(&pair.project, Some(&forged), claim.clone()))
            .await
            .unwrap(),
        "forbidden",
    );
    assert_eq!(pair.get_task(&queued.id).await, queued);
    // Even a real token cannot override the device identity of the transport.
    rejected(
        pair.host
            .collaborate(None, request(&pair.project, Some(&pair.auth), claim))
            .await
            .unwrap(),
        "forbidden",
    );
    assert_eq!(pair.get_task(&queued.id).await, queued);
    pair.claim(&queued).await;
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_session_credentials_cannot_claim() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    // The current store contract uses forbidden for absent and invalid credentials.
    // Keep this separate from forged-token assertions if that contract changes.
    rejected(
        pair.remote
            .collaborate(request(
                &pair.project,
                None,
                Action::TaskClaim {
                    task_id: queued.id.clone(),
                    expected_version: queued.version,
                },
            ))
            .await
            .unwrap(),
        "forbidden",
    );
    assert_eq!(pair.get_task(&queued.id).await, queued);
    pair.claim(&queued).await;
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revocation_is_checked_on_existing_connection_including_replay() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let claim = request(
        &pair.project,
        Some(&pair.auth),
        Action::TaskClaim {
            task_id: queued.id.clone(),
            expected_version: queued.version,
        },
    );
    let running = task(pair.remote.collaborate(claim.clone()).await.unwrap());
    response(
        pair.local(Action::MemberSet {
            device_id: pair.guest.id().to_string(),
            role: Role::Worker,
            revoked: true,
        })
        .await,
    );
    assert!(
        !pair.remote.is_closed(),
        "membership revocation must be tested on the same connection"
    );
    for operation in [
        request(
            &pair.project,
            Some(&pair.auth),
            Action::TaskGet {
                task_id: running.id.clone(),
            },
        ),
        request(
            &pair.project,
            Some(&pair.auth),
            Action::TaskProgress {
                task_id: running.id.clone(),
                attempt_id: running.attempts[0].id.clone(),
                message: "must be rejected".into(),
                blocked: false,
                expected_version: running.version,
            },
        ),
        claim,
    ] {
        rejected(
            pair.remote.collaborate(operation).await.unwrap(),
            "forbidden",
        );
    }
    assert_eq!(pair.get_task(&running.id).await, running);
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_replay_preserves_reply_and_rejects_changed_payload() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let claim = request(
        &pair.project,
        Some(&pair.auth),
        Action::TaskClaim {
            task_id: queued.id.clone(),
            expected_version: queued.version,
        },
    );
    let first = pair.remote.collaborate(claim.clone()).await.unwrap();
    let running = task(first.clone());
    let (_, cursor) = pair.events(0).await;
    assert_eq!(pair.remote.collaborate(claim.clone()).await.unwrap(), first);
    assert!(
        pair.events(cursor).await.0.is_empty(),
        "replay must not append another event"
    );
    let mut changed = claim;
    changed.action = Action::TaskProgress {
        task_id: running.id.clone(),
        attempt_id: running.attempts[0].id.clone(),
        message: "same ID with different contents".into(),
        blocked: false,
        expected_version: running.version,
    };
    rejected(
        pair.remote.collaborate(changed).await.unwrap(),
        "idempotency_conflict",
    );
    assert_eq!(pair.get_task(&running.id).await, running);
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_claims_create_only_one_attempt() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let action = Action::TaskClaim {
        task_id: queued.id.clone(),
        expected_version: queued.version,
    };
    let first = request(&pair.project, Some(&pair.auth), action.clone());
    let second = request(&pair.project, Some(&pair.auth), action);
    assert_ne!(first.request_id, second.request_id);
    let (a, b) = tokio::join!(
        pair.remote.collaborate(first),
        pair.remote.collaborate(second)
    );
    let replies = [a.unwrap(), b.unwrap()];
    assert_eq!(
        replies.iter().filter(|reply| reply.error.is_none()).count(),
        1,
        "{replies:?}"
    );
    let rejected_reply = replies.iter().find(|reply| reply.error.is_some()).unwrap();
    rejected(rejected_reply.clone(), "version_conflict");
    let winner = task(
        replies
            .iter()
            .find(|reply| reply.error.is_none())
            .unwrap()
            .clone(),
    );
    let stored = pair.get_task(&queued.id).await;
    assert_eq!(stored, winner);
    assert_eq!(stored.attempts.len(), 1);
    assert_eq!(stored.state, TaskState::Running);
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_old_attempt_result_cannot_replace_requeued_attempt() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let running = pair.claim(&queued).await;
    let old_attempt = running.attempts[0].id.clone();
    let cancelling = task(
        pair.local(Action::TaskCancel {
            task_id: running.id.clone(),
            expected_version: running.version,
        })
        .await,
    );
    assert_eq!(cancelling.state, TaskState::CancelRequested);
    let stopped = task(
        pair.worker(Action::TaskStopAck {
            task_id: running.id.clone(),
            attempt_id: old_attempt.clone(),
            expected_version: cancelling.version,
        })
        .await,
    );
    assert_eq!(stopped.state, TaskState::Cancelled);
    let requeued = task(
        pair.local(Action::TaskRequeue {
            task_id: running.id.clone(),
            expected_version: stopped.version,
        })
        .await,
    );
    let current = task(
        pair.worker(Action::TaskClaim {
            task_id: running.id.clone(),
            expected_version: requeued.version,
        })
        .await,
    );
    assert_eq!(current.attempts.len(), 2);
    assert_ne!(current.attempts[1].id, old_attempt);
    // Use the current version so the attempt check, not a stale version, protects the task.
    let late = pair
        .worker(Action::TaskResult {
            task_id: current.id.clone(),
            attempt_id: old_attempt,
            result: result("late old attempt"),
            expected_version: current.version,
        })
        .await;
    // A historical-only acknowledgement or a business rejection is permitted.
    assert!(
        late.error.is_some() || late.response.is_some(),
        "invalid reply: {late:?}"
    );
    let after = pair.get_task(&current.id).await;
    assert_eq!(after.state, TaskState::Running);
    assert_eq!(after.version, current.version);
    assert_eq!(after.attempts.last(), current.attempts.last());
    let ready = task(
        pair.worker(Action::TaskResult {
            task_id: current.id.clone(),
            attempt_id: current.attempts[1].id.clone(),
            result: result("current attempt"),
            expected_version: current.version,
        })
        .await,
    );
    assert_eq!(ready.state, TaskState::ResultReady);
    assert_eq!(
        ready.attempts[1].result.as_ref().unwrap().summary,
        "current attempt"
    );
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_retry_and_node_restart_keep_receipts_and_attempt() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let claim = request(
        &pair.project,
        Some(&pair.auth),
        Action::TaskClaim {
            task_id: queued.id.clone(),
            expected_version: queued.version,
        },
    );
    let committed = pair.remote.collaborate(claim.clone()).await.unwrap();
    let running = task(committed.clone());
    let (_, cursor) = pair.events(0).await;
    // Deterministic post-commit retry; this does not inject a lost QUIC response.
    pair.remote.close();
    tokio::time::timeout(Duration::from_secs(10), pair.remote.closed())
        .await
        .unwrap();
    let reconnected = connect(&pair.host, &pair.guest).await;
    assert_eq!(
        reconnected.collaborate(claim.clone()).await.unwrap(),
        committed
    );
    assert_eq!(pair.get_task(&running.id).await, running);
    assert!(pair.events(cursor).await.0.is_empty());
    reconnected.close();
    drop(reconnected);

    let Pair {
        host,
        guest,
        remote,
        project,
        worker: _,
        auth,
        dir,
    } = pair;
    let host_id = host.id();
    let guest_id = guest.id();
    guest.shutdown().await;
    host.shutdown().await;
    drop((remote, guest, host));
    let host = Node::start(dir.config("host"), Arc::new(Delegate))
        .await
        .unwrap();
    let guest = Node::start(dir.config("guest"), Arc::new(Delegate))
        .await
        .unwrap();
    assert_eq!(host.id(), host_id);
    assert_eq!(guest.id(), guest_id);
    let remote = connect(&host, &guest).await;
    assert_eq!(remote.collaborate(claim).await.unwrap(), committed);
    let read = remote
        .collaborate(request(
            &project,
            Some(&auth),
            Action::TaskGet {
                task_id: running.id.clone(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(task(read), running);
    match response(
        host.collaborate(
            None,
            request(
                &project,
                None,
                Action::EventsRead {
                    after: cursor,
                    limit: MAX_EVENTS,
                },
            ),
        )
        .await
        .unwrap(),
    ) {
        Response::Events {
            events,
            cursor: restored_cursor,
        } => {
            assert!(events.is_empty());
            assert_eq!(restored_cursor, cursor);
        }
        other => panic!("expected events, got {other:?}"),
    }
    let progress = task(
        remote
            .collaborate(request(
                &project,
                Some(&auth),
                Action::TaskProgress {
                    task_id: running.id.clone(),
                    attempt_id: running.attempts[0].id.clone(),
                    message: "continued after restart".into(),
                    blocked: false,
                    expected_version: running.version,
                },
            ))
            .await
            .unwrap(),
    );
    assert_eq!(progress.attempts.len(), 1);
    assert_eq!(
        progress.progress.as_deref(),
        Some("continued after restart")
    );
    remote.close();
    guest.shutdown().await;
    host.shutdown().await;
    drop((remote, guest, host, dir));
}

#[test]
fn legacy_handshake_defaults_to_no_collaboration_capability() {
    let hello: Msg = serde_json::from_value(json!({
        "t": "hello", "v": p2pcli_core::PROTO_VERSION, "name": "legacy",
        "platform": "test", "password": null
    }))
    .unwrap();
    let welcome: Msg = serde_json::from_value(json!({
        "t": "welcome", "name": "legacy", "platform": "test", "user": "fixture"
    }))
    .unwrap();
    for message in [hello, welcome] {
        match &message {
            Msg::Hello { capabilities, .. } | Msg::Welcome { capabilities, .. } => {
                assert!(capabilities.is_empty());
            }
            _ => unreachable!(),
        }
        let legacy = serde_json::to_value(&message).unwrap();
        assert!(legacy.get("capabilities").is_none());
        let mut modern = legacy;
        modern["capabilities"] = json!([CAPABILITY, "future_unknown_capability"]);
        let decoded: Msg = serde_json::from_value(modern).unwrap();
        match decoded {
            Msg::Hello { capabilities, .. } | Msg::Welcome { capabilities, .. } => {
                assert_eq!(capabilities, vec![CAPABILITY, "future_unknown_capability"]);
            }
            _ => unreachable!(),
        }
    }
}

fn message(reply: Reply) -> Message {
    match response(reply) {
        Response::Message { message } => message,
        other => panic!("expected message, got {other:?}"),
    }
}

fn messages(reply: Reply) -> Vec<Message> {
    match response(reply) {
        Response::Messages { messages } => messages,
        other => panic!("expected inbox, got {other:?}"),
    }
}

fn delivery_record(node: &Node, request: &Request) -> delivery::Delivery {
    let records = delivery::list(node.host.collaboration.data_dir()).unwrap();
    let matching: Vec<_> = records
        .into_iter()
        .filter(|record| record.request_id == request.request_id)
        .collect();
    assert_eq!(matching.len(), 1, "one durable record per operation");
    matching.into_iter().next().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delivery_replays_once_and_checks_membership_before_cached_reply() {
    let pair = Pair::new().await;
    let target = pair.host.id().to_string();
    let cached = pair.remote.clone();
    let (recipient, recipient_auth) = registered(pair.local(register_action("coordinator")).await);
    let send = request(
        &pair.project,
        Some(&pair.auth),
        Action::MessageSend {
            to: assignee(&recipient),
            task_id: None,
            body: "private message fixture".into(),
        },
    );
    let (a, b) = tokio::join!(
        delivery::dispatch(&pair.guest, Some(&target), send.clone()),
        delivery::dispatch(&pair.guest, Some(&target), send.clone()),
    );
    let first = a.unwrap();
    assert_eq!(b.unwrap(), first);
    let accepted = message(first.clone());
    assert!(!accepted.consumed, "delivered does not mean consumed");
    assert_eq!(accepted.from, assignee(&pair.worker));
    let inbox_request = request(&pair.project, Some(&recipient_auth), Action::Inbox);
    assert_eq!(
        messages(
            pair.host
                .collaborate(None, inbox_request.clone())
                .await
                .unwrap()
        ),
        vec![accepted.clone()]
    );
    assert_eq!(delivery_record(&pair.guest, &send).state, "delivered");
    let inspection =
        serde_json::to_string(&delivery::list(pair.guest.host.collaboration.data_dir()).unwrap())
            .unwrap();
    for secret in [&pair.auth.token, &accepted.body] {
        assert!(
            !inspection.contains(secret),
            "queue inspection leaked private contents"
        );
    }
    let mut changed = send.clone();
    if let Action::MessageSend { body, .. } = &mut changed.action {
        *body = "changed content".into();
    }
    assert!(
        delivery::dispatch(&pair.guest, Some(&target), changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("idempotency_conflict")
    );

    let (_, wrong_auth) = registered(pair.local(register_action("another local session")).await);
    rejected(
        pair.host
            .collaborate(
                None,
                request(
                    &pair.project,
                    Some(&wrong_auth),
                    Action::MessageConsume {
                        message_id: accepted.id.clone(),
                    },
                ),
            )
            .await
            .unwrap(),
        "forbidden",
    );
    rejected(
        pair.worker(Action::MessageConsume {
            message_id: accepted.id.clone(),
        })
        .await,
        "forbidden",
    );
    assert_eq!(
        messages(
            pair.host
                .collaborate(None, inbox_request.clone())
                .await
                .unwrap()
        ),
        vec![accepted.clone()]
    );
    let consume = request(
        &pair.project,
        Some(&recipient_auth),
        Action::MessageConsume {
            message_id: accepted.id.clone(),
        },
    );
    let consumed = delivery::dispatch(&pair.host, None, consume.clone())
        .await
        .unwrap();
    assert!(message(consumed.clone()).consumed);
    let (_, cursor) = pair.events(0).await;
    assert_eq!(
        delivery::dispatch(&pair.host, None, consume).await.unwrap(),
        consumed
    );
    assert!(pair.events(cursor).await.0.is_empty());

    // Cache a credential-bearing response too; it must not escape after revocation.
    let registration = request(&pair.project, None, register_action("cached registration"));
    registered(
        delivery::dispatch(&pair.guest, Some(&target), registration.clone())
            .await
            .unwrap(),
    );
    response(
        pair.local(Action::MemberSet {
            device_id: pair.guest.id().to_string(),
            role: Role::Worker,
            revoked: true,
        })
        .await,
    );
    assert!(!cached.is_closed());
    for operation in [send.clone(), registration] {
        rejected(
            delivery::dispatch(&pair.guest, Some(&target), operation)
                .await
                .unwrap(),
            "forbidden",
        );
    }
    assert_eq!(
        messages(pair.host.collaborate(None, inbox_request).await.unwrap()).len(),
        1
    );
    cached.close();
    drop(cached);
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_delivery_survives_restart_but_claim_requires_explicit_retry() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let (recipient, recipient_auth) = registered(pair.local(register_action("recipient")).await);
    let send = request(
        &pair.project,
        Some(&pair.auth),
        Action::MessageSend {
            to: assignee(&recipient),
            task_id: Some(queued.id.clone()),
            body: "queued while sender transport is closed".into(),
        },
    );
    let claim = request(
        &pair.project,
        Some(&pair.auth),
        Action::TaskClaim {
            task_id: queued.id.clone(),
            expected_version: queued.version,
        },
    );
    let target = pair.host.id().to_string();
    // Closing the local transport produces a deterministic send failure without
    // waiting for internet routing or racing a remote commit.
    pair.guest.endpoint.close().await;
    let pending = tokio::time::timeout(
        Duration::from_secs(5),
        delivery::dispatch(&pair.guest, Some(&target), send.clone()),
    )
    .await
    .expect("closed endpoint should fail promptly")
    .unwrap();
    assert!(
        matches!(response(pending), Response::Queued { request_id, .. } if request_id == send.request_id)
    );
    assert!(
        tokio::time::timeout(
            Duration::from_secs(5),
            delivery::dispatch(&pair.guest, Some(&target), claim.clone())
        )
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(delivery_record(&pair.guest, &send).state, "pending");
    let claim_record = delivery_record(&pair.guest, &claim);
    assert_eq!(claim_record.state, "needs_retry");
    assert_eq!(pair.get_task(&queued.id).await, queued);
    assert!(
        messages(
            pair.host
                .collaborate(
                    None,
                    request(&pair.project, Some(&recipient_auth), Action::Inbox)
                )
                .await
                .unwrap()
        )
        .is_empty()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let queue = pair
            .guest
            .host
            .collaboration
            .data_dir()
            .join("collaboration-outbox");
        assert_eq!(
            std::fs::metadata(&queue).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in std::fs::read_dir(queue).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    let Pair {
        host,
        guest,
        remote,
        project,
        worker: _,
        auth: _,
        dir,
    } = pair;
    let guest_id = guest.id();
    guest.shutdown().await;
    drop((remote, guest));
    let guest = Node::start(dir.config("guest"), Arc::new(Delegate))
        .await
        .unwrap();
    assert_eq!(guest.id(), guest_id);
    let remote = Arc::new(connect(&host, &guest).await);
    guest.host.paired.add(&host.id(), "host", "test").unwrap();
    guest.adopt_remote(remote.clone());
    // Observe the public queue state; no fixture writes to the implementation's files.
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            if delivery_record(&guest, &send).state == "delivered" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("background delivery did not resume after sender restart");
    assert!(delivery_record(&guest, &send).attempts >= 2);
    assert_eq!(delivery_record(&guest, &claim).state, "needs_retry");
    assert_eq!(
        delivery_record(&guest, &claim).attempts,
        claim_record.attempts
    );
    assert_eq!(
        task(
            host.collaborate(
                None,
                request(
                    &project,
                    None,
                    Action::TaskGet {
                        task_id: queued.id.clone()
                    }
                )
            )
            .await
            .unwrap()
        ),
        queued
    );
    let inbox = messages(
        host.collaborate(
            None,
            request(&project, Some(&recipient_auth), Action::Inbox),
        )
        .await
        .unwrap(),
    );
    assert_eq!(inbox.len(), 1);
    assert!(!inbox[0].consumed);
    let replay = delivery::dispatch(&guest, Some(&target), send.clone())
        .await
        .unwrap();
    assert_eq!(message(replay.clone()), inbox[0]);
    let running = task(
        delivery::dispatch(&guest, Some(&target), claim)
            .await
            .unwrap(),
    );
    assert_eq!(running.state, TaskState::Running);
    assert_eq!(running.attempts.len(), 1);

    // Restart again with the delivered receipt already on disk.
    remote.close();
    guest.shutdown().await;
    drop((remote, guest));
    let guest = Node::start(dir.config("guest"), Arc::new(Delegate))
        .await
        .unwrap();
    let remote = Arc::new(connect(&host, &guest).await);
    guest.host.paired.add(&host.id(), "host", "test").unwrap();
    guest.adopt_remote(remote.clone());
    assert_eq!(
        delivery::dispatch(&guest, Some(&target), send)
            .await
            .unwrap(),
        replay
    );
    assert_eq!(
        messages(
            host.collaborate(
                None,
                request(&project, Some(&recipient_auth), Action::Inbox)
            )
            .await
            .unwrap()
        ),
        inbox
    );
    remote.close();
    guest.shutdown().await;
    host.shutdown().await;
    drop((remote, guest, host, dir));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_delivery_and_read_requests_do_not_create_retryable_work() {
    let pair = Pair::new().await;
    let target = pair.host.id().to_string();
    let remote = pair.remote.clone();
    response(
        delivery::dispatch(
            &pair.guest,
            Some(&target),
            request(&pair.project, None, Action::ProjectGet),
        )
        .await
        .unwrap(),
    );
    assert!(
        delivery::list(pair.guest.host.collaboration.data_dir())
            .unwrap()
            .is_empty()
    );
    let escalate = request(
        &pair.project,
        Some(&pair.auth),
        Action::MemberSet {
            device_id: pair.guest.id().to_string(),
            role: Role::Admin,
            revoked: false,
        },
    );
    rejected(
        delivery::dispatch(&pair.guest, Some(&target), escalate.clone())
            .await
            .unwrap(),
        "forbidden",
    );
    let record = delivery_record(&pair.guest, &escalate);
    assert_eq!(record.state, "rejected");
    rejected(
        delivery::dispatch(&pair.guest, Some(&target), escalate.clone())
            .await
            .unwrap(),
        "forbidden",
    );
    assert_eq!(
        delivery_record(&pair.guest, &escalate).attempts,
        record.attempts
    );
    match response(pair.local(Action::MembersList).await) {
        Response::Members { members } => assert_eq!(
            members
                .iter()
                .find(|member| member.device_id == pair.guest.id().to_string())
                .unwrap()
                .role,
            Role::Worker,
        ),
        other => panic!("expected members, got {other:?}"),
    }
    remote.close();
    drop(remote);
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_scope_and_role_changes_are_enforced_without_reconnecting() {
    let pair = Pair::new().await;
    let queued = pair.create_task().await;
    let (_, other_auth) = registered(
        pair.remote
            .collaborate(request(
                &pair.project,
                None,
                register_action("another worker"),
            ))
            .await
            .unwrap(),
    );
    rejected(
        pair.remote
            .collaborate(request(
                &pair.project,
                Some(&other_auth),
                Action::TaskClaim {
                    task_id: queued.id.clone(),
                    expected_version: queued.version,
                },
            ))
            .await
            .unwrap(),
        "forbidden",
    );
    assert_eq!(pair.get_task(&queued.id).await, queued);
    response(
        pair.local(Action::MemberSet {
            device_id: pair.guest.id().to_string(),
            role: Role::Viewer,
            revoked: false,
        })
        .await,
    );
    rejected(
        pair.worker(Action::TaskClaim {
            task_id: queued.id.clone(),
            expected_version: queued.version,
        })
        .await,
        "forbidden",
    );
    rejected(
        pair.worker(Action::MemberSet {
            device_id: pair.guest.id().to_string(),
            role: Role::Admin,
            revoked: false,
        })
        .await,
        "forbidden",
    );
    assert_eq!(pair.get_task(&queued.id).await, queued);
    response(
        pair.local(Action::MemberSet {
            device_id: pair.guest.id().to_string(),
            role: Role::Worker,
            revoked: false,
        })
        .await,
    );
    pair.claim(&queued).await;
    pair.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_reconnection_cannot_bypass_capabilities_with_cached_delivery() {
    use p2pcli_core::proto::{read_msg, write_json, write_msg};

    let dir = TestDir::new();
    let guest = Node::start(dir.config("guest"), Arc::new(Delegate))
        .await
        .unwrap();
    // A scripted peer isolates capability negotiation from domain authorization.
    // It first advertises support, then emulates an older version on reconnect.
    let server = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .alpns(vec![p2pcli_core::ALPN_TERM.to_vec()])
        .clear_address_lookup()
        .relay_mode(iroh::RelayMode::Disabled)
        .bind_addr((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .bind()
        .await
        .unwrap();
    let address = EndpointAddr::from_parts(
        server.id(),
        server.bound_sockets().into_iter().map(TransportAddr::Ip),
    );
    let target = server.id().to_string();
    let operation = Request::new(
        Some("capability-fixture".into()),
        Action::MemberSet {
            device_id: guest.id().to_string(),
            role: Role::Viewer,
            revoked: false,
        },
    );
    let expected_operation = operation.clone();
    let peer_endpoint = server.clone();
    let peer = tokio::spawn(async move {
        let modern = peer_endpoint.accept().await.unwrap().await.unwrap();
        let (mut control_send, mut control_recv) = modern.accept_bi().await.unwrap();
        assert!(matches!(
            read_msg(&mut control_recv).await.unwrap(),
            Msg::Hello { .. }
        ));
        write_json(
            &mut control_send,
            &json!({
                "t": "welcome", "name": "caps-fixture", "platform": "test", "user": "fixture",
                "capabilities": [CAPABILITY]
            }),
        )
        .await
        .unwrap();
        let (mut send, mut recv) = modern.accept_bi().await.unwrap();
        assert_eq!(
            read_msg(&mut recv).await.unwrap(),
            Msg::CollabRequest {
                request: expected_operation
            }
        );
        write_msg(
            &mut send,
            &Msg::CollabReply {
                reply: Reply::ok(Response::Ack),
            },
        )
        .await
        .unwrap();
        send.finish().unwrap();
        let legacy = peer_endpoint.accept().await.unwrap().await.unwrap();
        let (mut legacy_send, mut legacy_recv) = legacy.accept_bi().await.unwrap();
        assert!(matches!(
            read_msg(&mut legacy_recv).await.unwrap(),
            Msg::Hello { .. }
        ));
        write_json(
            &mut legacy_send,
            &json!({
                "t": "welcome", "name": "caps-fixture", "platform": "test", "user": "fixture"
            }),
        )
        .await
        .unwrap();
        // A sentinel existing operation proves no collaboration stream slipped through.
        let (mut send, mut recv) = legacy.accept_bi().await.unwrap();
        assert_eq!(read_msg(&mut recv).await.unwrap(), Msg::ListTerminals);
        write_msg(&mut send, &Msg::Terminals { sessions: vec![] })
            .await
            .unwrap();
        send.finish().unwrap();
        (
            modern,
            control_send,
            control_recv,
            legacy,
            legacy_send,
            legacy_recv,
        )
    });
    let modern = Arc::new(guest.connect_addr(address.clone(), None).await.unwrap());
    guest
        .host
        .paired
        .add(&server.id(), "caps-fixture", "test")
        .unwrap();
    guest.adopt_remote(modern.clone());
    assert_eq!(
        delivery::dispatch(&guest, Some(&target), operation.clone())
            .await
            .unwrap(),
        Reply::ok(Response::Ack)
    );
    assert_eq!(delivery_record(&guest, &operation).state, "delivered");
    modern.close();
    let legacy = Arc::new(guest.connect_addr(address, None).await.unwrap());
    guest.adopt_remote(legacy.clone());
    for error in [
        delivery::dispatch(&guest, Some(&target), operation.clone())
            .await
            .unwrap_err(),
        guest
            .collaborate(Some(&target), operation.clone())
            .await
            .unwrap_err(),
    ] {
        assert!(
            error.to_string().contains("unsupported_capability"),
            "{error:#}"
        );
    }
    let bypass = legacy
        .open_stream(&Msg::CollabRequest { request: operation })
        .await;
    assert!(
        bypass.is_err(),
        "generic stream must enforce negotiated capabilities"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), legacy.list_terminals())
            .await
            .unwrap()
            .unwrap()
            .is_empty()
    );
    let controls = tokio::time::timeout(Duration::from_secs(5), peer)
        .await
        .unwrap()
        .unwrap();
    legacy.close();
    guest.shutdown().await;
    server.close().await;
    drop((controls, modern, legacy, guest, server, dir));
}
