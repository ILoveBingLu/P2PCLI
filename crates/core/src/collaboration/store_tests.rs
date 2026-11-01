use super::*;
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp_dir(label: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("p2pcli-collab-{label}-{suffix}"));
    fs::create_dir_all(&path).unwrap();
    path
}

fn request(project_id: Option<&str>, action: Action) -> Request {
    Request::new(project_id.map(str::to_owned), action)
}

fn response(reply: Reply) -> Response {
    reply.into_result().unwrap()
}

fn project(store: &Store, actor: &str) -> Project {
    match response(store.process(
        actor,
        &request(
            None,
            Action::ProjectCreate {
                name: "test".into(),
            },
        ),
    )) {
        Response::Project { project } => project,
        other => panic!("unexpected response: {other:?}"),
    }
}

fn add_member(store: &Store, project: &Project, actor: &str, device: &str, role: Role) {
    let reply = store.process(
        actor,
        &Request {
            request_id: new_id(),
            project_id: Some(project.id.clone()),
            auth: None,
            action: Action::MemberSet {
                device_id: device.into(),
                role,
                revoked: false,
            },
        },
    );
    assert!(reply.error.is_none(), "{reply:?}");
}

fn register(store: &Store, project: &Project, actor: &str) -> (AgentSession, String) {
    match response(store.process(
        actor,
        &Request {
            request_id: new_id(),
            project_id: Some(project.id.clone()),
            auth: None,
            action: Action::SessionRegister {
                name: actor.into(),
                mode: SessionMode::ManualPull,
                terminal: None,
                cwd: None,
            },
        },
    )) {
        Response::Registered { session, token } => (session, token),
        other => panic!("unexpected response: {other:?}"),
    }
}

fn task_request(project: &Project, action: Action, auth: Option<SessionAuth>) -> Request {
    Request {
        request_id: new_id(),
        project_id: Some(project.id.clone()),
        auth,
        action,
    }
}

fn create_task(store: &Store, project: &Project, actor: &str, assignee: SessionRef) -> Task {
    match response(store.process(
        actor,
        &task_request(
            project,
            Action::TaskCreate {
                spec: TaskSpec {
                    title: "verify".into(),
                    goal: "run checks".into(),
                    acceptance: "green".into(),
                    inputs: Value::Null,
                    assignee,
                },
            },
            None,
        ),
    )) {
        Response::Task { task } => task,
        other => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn idempotency_replays_and_rejects_conflicting_payload() {
    let dir = temp_dir("idempotency");
    let store = Store::open(&dir, "admin").unwrap();
    let request_id = "same-operation".to_owned();
    let first = Request {
        request_id: request_id.clone(),
        project_id: None,
        auth: None,
        action: Action::ProjectCreate { name: "one".into() },
    };
    let replay = store.process("admin", &first);
    assert!(replay.error.is_none());
    assert_eq!(store.process("admin", &first), replay);
    let second = Request {
        action: Action::ProjectCreate { name: "two".into() },
        ..first
    };
    let conflict = store.process("admin", &second);
    assert_eq!(conflict.error.unwrap().code, "idempotency_conflict");
    let projects = response(store.process("admin", &request(None, Action::ProjectsList)));
    assert_eq!(
        match projects {
            Response::Projects { projects } => projects.len(),
            _ => 0,
        },
        1
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn two_claimers_only_one_receive_attempt() {
    let dir = temp_dir("claim");
    let store = Store::open(&dir, "admin").unwrap();
    let project = project(&store, "admin");
    add_member(&store, &project, "admin", "worker", Role::Worker);
    let (session, token) = register(&store, &project, "worker");
    let task = create_task(
        &store,
        &project,
        "admin",
        SessionRef {
            device_id: "worker".into(),
            session_id: session.id.clone(),
        },
    );
    let claim = Action::TaskClaim {
        task_id: task.id.clone(),
        expected_version: 1,
    };
    let auth = Some(SessionAuth {
        session_id: session.id,
        token,
    });
    let a = Arc::new(store);
    let b = Arc::clone(&a);
    let auth_a = auth.clone();
    let t = task.clone();
    let project_a = project.clone();
    let first =
        thread::spawn(move || a.process("worker", &task_request(&project_a, claim, auth_a)));
    let second = thread::spawn(move || {
        b.process(
            "worker",
            &task_request(
                &project,
                Action::TaskClaim {
                    task_id: t.id,
                    expected_version: 1,
                },
                auth,
            ),
        )
    });
    let replies = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(
        replies.iter().filter(|reply| reply.error.is_none()).count(),
        1
    );
    assert_eq!(
        replies
            .iter()
            .filter_map(|reply| reply.error.as_ref().map(|error| error.code.as_str()))
            .count(),
        1
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn revoked_member_cannot_reuse_session_or_read_project() {
    let dir = temp_dir("revoke");
    let store = Store::open(&dir, "admin");
    let store = store.unwrap();
    let project = project(&store, "admin");
    add_member(&store, &project, "admin", "worker", Role::Worker);
    let (session, token) = register(&store, &project, "worker");
    let revoke = store.process(
        "admin",
        &task_request(
            &project,
            Action::MemberSet {
                device_id: "worker".into(),
                role: Role::Worker,
                revoked: true,
            },
            None,
        ),
    );
    assert!(revoke.error.is_none());
    let denied = store.process(
        "worker",
        &task_request(
            &project,
            Action::Inbox,
            Some(SessionAuth {
                session_id: session.id,
                token,
            }),
        ),
    );
    assert_eq!(denied.error.unwrap().code, "forbidden");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn records_survive_restart_and_cancel_race_keeps_result() {
    let dir = temp_dir("restart");
    let (task_id, attempt_id, project_id, token, session_id);
    {
        let store = Store::open(&dir, "admin").unwrap();
        let project = project(&store, "admin");
        add_member(&store, &project, "admin", "worker", Role::Worker);
        let (session, session_token) = register(&store, &project, "worker");
        let task = create_task(
            &store,
            &project,
            "admin",
            SessionRef {
                device_id: "worker".into(),
                session_id: session.id.clone(),
            },
        );
        let claimed = match response(store.process(
            "worker",
            &task_request(
                &project,
                Action::TaskClaim {
                    task_id: task.id.clone(),
                    expected_version: 1,
                },
                Some(SessionAuth {
                    session_id: session.id.clone(),
                    token: session_token.clone(),
                }),
            ),
        )) {
            Response::Task { task } => task,
            other => panic!("unexpected response: {other:?}"),
        };
        let attempt = claimed.attempts.last().unwrap().id.clone();
        let cancelled = store.process(
            "admin",
            &task_request(
                &project,
                Action::TaskCancel {
                    task_id: task.id.clone(),
                    expected_version: claimed.version,
                },
                None,
            ),
        );
        assert!(cancelled.error.is_none());
        task_id = task.id;
        attempt_id = attempt;
        project_id = project.id;
        token = session_token;
        session_id = session.id;
    }
    let store = Store::open(&dir, "admin").unwrap();
    let result = store.process(
        "worker",
        &task_request(
            &Project {
                id: project_id.clone(),
                name: "test".into(),
                coordinator_id: "admin".into(),
                created_at: 0,
            },
            Action::TaskResult {
                task_id: task_id.clone(),
                attempt_id,
                expected_version: 3,
                result: TaskResult {
                    summary: "finished before stop".into(),
                    completed: vec![],
                    remaining: vec![],
                    verification: vec![],
                    artifact_ids: vec![],
                    failed: false,
                },
            },
            Some(SessionAuth { session_id, token }),
        ),
    );
    assert!(result.error.is_none(), "{result:?}");
    let task = match response(store.process(
        "admin",
        &task_request(
            &Project {
                id: project_id.clone(),
                name: "test".into(),
                coordinator_id: "admin".into(),
                created_at: 0,
            },
            Action::TaskGet { task_id },
            None,
        ),
    )) {
        Response::Task { task } => task,
        other => panic!("unexpected response: {other:?}"),
    };
    assert_eq!(task.state, TaskState::ResultReady);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stale_attempt_and_missing_artifact_block_completion() {
    let dir = temp_dir("artifact");
    let store = Store::open(&dir, "admin").unwrap();
    let project = project(&store, "admin");
    add_member(&store, &project, "admin", "worker", Role::Worker);
    let (session, token) = register(&store, &project, "worker");
    let auth = Some(SessionAuth {
        session_id: session.id.clone(),
        token,
    });
    let task = create_task(
        &store,
        &project,
        "admin",
        SessionRef {
            device_id: "worker".into(),
            session_id: session.id.clone(),
        },
    );
    let task = match response(store.process(
        "worker",
        &task_request(
            &project,
            Action::TaskClaim {
                task_id: task.id.clone(),
                expected_version: 1,
            },
            auth.clone(),
        ),
    )) {
        Response::Task { task } => task,
        other => panic!("unexpected response: {other:?}"),
    };
    let attempt_id = task.attempts.last().unwrap().id.clone();
    let artifact = match response(store.process(
        "worker",
        &task_request(
            &project,
            Action::ArtifactDeclare {
                task_id: task.id.clone(),
                attempt_id: attempt_id.clone(),
                name: "result.txt".into(),
                size: 3,
                hash: blake3::hash(b"yes").to_hex().to_string(),
            },
            auth.clone(),
        ),
    )) {
        Response::Artifact { artifact } => artifact,
        other => panic!("unexpected response: {other:?}"),
    };
    let result = store.process(
        "worker",
        &task_request(
            &project,
            Action::TaskResult {
                task_id: task.id.clone(),
                attempt_id: attempt_id.clone(),
                expected_version: task.version,
                result: TaskResult {
                    summary: "done".into(),
                    completed: vec![],
                    remaining: vec![],
                    verification: vec![],
                    artifact_ids: vec![artifact.id],
                    failed: false,
                },
            },
            auth.clone(),
        ),
    );
    let task = match response(result) {
        Response::Task { task } => task,
        other => panic!("unexpected response: {other:?}"),
    };
    let denied = store.process(
        "admin",
        &task_request(
            &project,
            Action::TaskAccept {
                task_id: task.id.clone(),
                expected_version: task.version,
            },
            None,
        ),
    );
    assert_eq!(denied.error.unwrap().code, "artifact_pending");
    let stale = store.process(
        "worker",
        &task_request(
            &project,
            Action::TaskProgress {
                task_id: task.id,
                attempt_id: "old-attempt".into(),
                message: "late".into(),
                blocked: false,
                expected_version: 3,
            },
            auth,
        ),
    );
    assert_eq!(stale.error.unwrap().code, "stale_attempt");
    fs::remove_dir_all(dir).unwrap();
}

struct Fixture {
    store: Store,
    project: Project,
    session: AgentSession,
    auth: SessionAuth,
    task: Task,
    dir: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let dir = temp_dir(label);
        let store = Store::open(&dir, "admin").unwrap();
        let project = project(&store, "admin");
        add_member(&store, &project, "admin", "worker", Role::Worker);
        let (session, token) = register(&store, &project, "worker");
        let auth = SessionAuth {
            session_id: session.id.clone(),
            token,
        };
        let task = create_task(
            &store,
            &project,
            "admin",
            SessionRef {
                device_id: "worker".into(),
                session_id: session.id.clone(),
            },
        );
        Self {
            store,
            project,
            session,
            auth,
            task,
            dir,
        }
    }

    fn admin(&self, action: Action) -> Reply {
        self.store
            .process("admin", &task_request(&self.project, action, None))
    }

    fn worker(&self, action: Action) -> Reply {
        self.store.process(
            "worker",
            &task_request(&self.project, action, Some(self.auth.clone())),
        )
    }

    fn claim(&self) -> Task {
        as_task(self.worker(Action::TaskClaim {
            task_id: self.task.id.clone(),
            expected_version: self.task.version,
        }))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Best effort: Windows may still hold the SQLite file until Store drops.
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn as_task(reply: Reply) -> Task {
    match response(reply) {
        Response::Task { task } => task,
        other => panic!("expected task: {other:?}"),
    }
}

fn result_data(artifacts: Vec<String>) -> TaskResult {
    TaskResult {
        summary: "finished".into(),
        completed: vec!["verification".into()],
        remaining: vec![],
        verification: vec!["test passed".into()],
        artifact_ids: artifacts,
        failed: false,
    }
}

fn assert_error(reply: Reply, code: &str) {
    assert_eq!(
        reply.error.as_ref().map(|error| error.code.as_str()),
        Some(code),
        "{reply:?}"
    );
    assert!(reply.response.is_none());
}

#[test]
fn artifact_receipt_verifies_bytes_and_accept_rechecks_changed_file() {
    let fixture = Fixture::new("artifact-verification");
    let running = fixture.claim();
    let attempt = running.attempts.last().unwrap();
    let artifact = match response(fixture.worker(Action::ArtifactDeclare {
        task_id: running.id.clone(),
        attempt_id: attempt.id.clone(),
        name: "result.log".into(),
        size: 3,
        hash: blake3::hash(b"yes").to_hex().to_string(),
    })) {
        Response::Artifact { artifact } => artifact,
        other => panic!("{other:?}"),
    };
    let path = fixture.dir.join("result.log");
    let receipt = Action::ArtifactReceipt {
        artifact_id: artifact.id.clone(),
        local_path: path.to_string_lossy().into_owned(),
    };
    assert_error(fixture.worker(receipt.clone()), "forbidden");
    assert_error(fixture.admin(receipt.clone()), "artifact_pending");
    fs::write(&path, b"bad").unwrap();
    assert_error(fixture.admin(receipt.clone()), "artifact_mismatch");
    fs::write(&path, b"yes").unwrap();
    response(fixture.admin(receipt));
    let ready = as_task(fixture.worker(Action::TaskResult {
        task_id: running.id.clone(),
        attempt_id: attempt.id.clone(),
        result: result_data(vec![artifact.id]),
        expected_version: running.version,
    }));
    fs::write(&path, b"bad").unwrap();
    let accept = Action::TaskAccept {
        task_id: ready.id.clone(),
        expected_version: ready.version,
    };
    assert_error(fixture.admin(accept.clone()), "artifact_mismatch");
    assert_eq!(
        as_task(fixture.admin(Action::TaskGet { task_id: ready.id })).state,
        TaskState::ResultReady
    );
    fs::write(&path, b"yes").unwrap();
    assert_eq!(as_task(fixture.admin(accept)).state, TaskState::Completed);
}

#[test]
fn message_delivery_requires_addressed_session_and_consumption_proves_delivery() {
    let fixture = Fixture::new("messages");
    let (sender, token) = register(&fixture.store, &fixture.project, "admin");
    let sender_auth = SessionAuth {
        session_id: sender.id,
        token,
    };
    let send = || match response(fixture.store.process(
        "admin",
        &task_request(
            &fixture.project,
            Action::MessageSend {
                to: SessionRef {
                    device_id: "worker".into(),
                    session_id: fixture.session.id.clone(),
                },
                task_id: Some(fixture.task.id.clone()),
                body: "private message".into(),
            },
            Some(sender_auth.clone()),
        ),
    )) {
        Response::Message { message } => message,
        other => panic!("{other:?}"),
    };
    let message = send();
    assert!(!message.delivered);
    assert!(!message.consumed);
    let action = Action::MessageDelivered {
        message_id: message.id.clone(),
    };
    assert_error(
        fixture.store.process(
            "worker",
            &task_request(&fixture.project, action.clone(), None),
        ),
        "forbidden",
    );
    assert_error(
        fixture.store.process(
            "admin",
            &task_request(&fixture.project, action.clone(), Some(sender_auth.clone())),
        ),
        "forbidden",
    );
    let (wrong_session, wrong_token) = register(&fixture.store, &fixture.project, "worker");
    assert_error(
        fixture.store.process(
            "worker",
            &task_request(
                &fixture.project,
                action.clone(),
                Some(SessionAuth {
                    session_id: wrong_session.id,
                    token: wrong_token,
                }),
            ),
        ),
        "forbidden",
    );
    let delivered = fixture.worker(action);
    assert!(
        matches!(response(delivered), Response::Message { message } if message.delivered && !message.consumed)
    );
    let consumed = fixture.worker(Action::MessageConsume {
        message_id: message.id,
    });
    assert!(
        matches!(response(consumed), Response::Message { message } if message.delivered && message.consumed)
    );
    let second = send();
    assert!(
        matches!(response(fixture.worker(Action::MessageConsume { message_id: second.id })),
        Response::Message { message } if message.delivered && message.consumed)
    );
}

#[test]
fn worker_events_filter_old_ownership_private_messages_and_advance_scan_cursor() {
    let fixture = Fixture::new("event-privacy");
    add_member(
        &fixture.store,
        &fixture.project,
        "admin",
        "other",
        Role::Worker,
    );
    let (other, other_token) = register(&fixture.store, &fixture.project, "other");
    let (sender, sender_token) = register(&fixture.store, &fixture.project, "admin");
    let private = fixture.store.process(
        "admin",
        &task_request(
            &fixture.project,
            Action::MessageSend {
                to: SessionRef {
                    device_id: other.device_id.clone(),
                    session_id: other.id.clone(),
                },
                task_id: None,
                body: "OTHER-WORKER-PRIVATE".into(),
            },
            Some(SessionAuth {
                session_id: sender.id,
                token: sender_token,
            }),
        ),
    );
    response(private);
    let before = match response(fixture.worker(Action::EventsRead {
        after: 0,
        limit: MAX_EVENTS,
    })) {
        Response::Events { events, cursor } => {
            assert!(!encode(&events).unwrap().contains("OTHER-WORKER-PRIVATE"));
            assert!(
                cursor > events.last().unwrap().seq,
                "cursor must scan invisible tail"
            );
            cursor
        }
        other => panic!("{other:?}"),
    };
    let assigned = as_task(fixture.admin(Action::TaskAssign {
        task_id: fixture.task.id.clone(),
        assignee: SessionRef {
            device_id: "other".into(),
            session_id: other.id.clone(),
        },
        expected_version: fixture.task.version,
    }));
    let old_events = response(fixture.worker(Action::EventsRead {
        after: 0,
        limit: MAX_EVENTS,
    }));
    match old_events {
        Response::Events { events, cursor } => {
            assert!(
                events
                    .iter()
                    .all(|event| event.entity_id != fixture.task.id)
            );
            assert!(cursor > before);
        }
        other => panic!("{other:?}"),
    }
    let new_events = response(fixture.store.process(
        "other",
        &task_request(
            &fixture.project,
            Action::EventsRead {
                after: 0,
                limit: MAX_EVENTS,
            },
            Some(SessionAuth {
                session_id: other.id,
                token: other_token,
            }),
        ),
    ));
    assert!(matches!(new_events, Response::Events { events, .. }
        if events.iter().filter(|event| event.entity_id == assigned.id).all(|event| event.kind == "task_assigned")));
}

#[test]
fn revoked_session_stays_invalid_after_member_is_reauthorized() {
    let fixture = Fixture::new("reauthorize");
    let claim = task_request(
        &fixture.project,
        Action::TaskClaim {
            task_id: fixture.task.id.clone(),
            expected_version: fixture.task.version,
        },
        Some(fixture.auth.clone()),
    );
    response(fixture.store.process("worker", &claim));
    response(fixture.admin(Action::MemberSet {
        device_id: "worker".into(),
        role: Role::Worker,
        revoked: true,
    }));
    assert_error(fixture.store.process("worker", &claim), "forbidden");
    response(fixture.admin(Action::MemberSet {
        device_id: "worker".into(),
        role: Role::Worker,
        revoked: false,
    }));
    assert_error(fixture.store.process("worker", &claim), "forbidden");
    assert_error(
        fixture.worker(Action::SessionHeartbeat { available: true }),
        "forbidden",
    );
}

#[test]
fn independent_connections_claim_with_atomic_compare_and_swap_and_replay_after_restart() {
    let fixture = Fixture::new("connections");
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Store>();
    let second = Store::open(&fixture.dir, "admin").unwrap();
    let first_request = task_request(
        &fixture.project,
        Action::TaskClaim {
            task_id: fixture.task.id.clone(),
            expected_version: fixture.task.version,
        },
        Some(fixture.auth.clone()),
    );
    let second_request = Request {
        request_id: new_id(),
        ..first_request.clone()
    };
    let barrier = std::sync::Barrier::new(2);
    let (first, second_reply) = thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            fixture.store.process("worker", &first_request)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            second.process("worker", &second_request)
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_ne!(first.error.is_none(), second_reply.error.is_none());
    let (winning_request, winning_reply) = if first.error.is_none() {
        (first_request, first)
    } else {
        (second_request, second_reply)
    };
    let reopened = Store::open(&fixture.dir, "admin").unwrap();
    assert_eq!(reopened.process("worker", &winning_request), winning_reply);
    assert_eq!(
        as_task(fixture.admin(Action::TaskGet {
            task_id: fixture.task.id.clone()
        }))
        .attempts
        .len(),
        1
    );
}

#[test]
fn sql_failure_rolls_back_task_attempt_event_and_receipt() {
    let fixture = Fixture::new("rollback");
    fixture.store.connection.lock().unwrap().execute_batch(
        "CREATE TRIGGER fail_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT, 'test failure'); END;"
    ).unwrap();
    let claim = task_request(
        &fixture.project,
        Action::TaskClaim {
            task_id: fixture.task.id.clone(),
            expected_version: fixture.task.version,
        },
        Some(fixture.auth.clone()),
    );
    assert_error(fixture.store.process("worker", &claim), "storage_error");
    assert_eq!(
        as_task(fixture.admin(Action::TaskGet {
            task_id: fixture.task.id.clone()
        })),
        fixture.task
    );
    let connection = fixture.store.connection.lock().unwrap();
    let attempts: i64 = connection
        .query_row("SELECT count(*) FROM attempts", [], |row| row.get(0))
        .unwrap();
    let receipts: i64 = connection
        .query_row(
            "SELECT count(*) FROM receipts WHERE operation_id = ?1",
            [&claim.request_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((attempts, receipts), (0, 0));
    connection.execute_batch("DROP TRIGGER fail_event").unwrap();
    drop(connection);
    assert_eq!(
        as_task(fixture.store.process("worker", &claim)).state,
        TaskState::Running
    );
}

#[test]
fn late_result_is_historical_and_cannot_replace_new_attempt() {
    let fixture = Fixture::new("old-attempt");
    let running = fixture.claim();
    let cancelled = as_task(fixture.admin(Action::TaskCancel {
        task_id: running.id.clone(),
        expected_version: running.version,
    }));
    assert_error(
        fixture.admin(Action::TaskRequeue {
            task_id: running.id.clone(),
            expected_version: cancelled.version,
        }),
        "invalid_state",
    );
    let stopped = as_task(fixture.worker(Action::TaskStopAck {
        task_id: running.id.clone(),
        attempt_id: running.attempts[0].id.clone(),
        expected_version: cancelled.version,
    }));
    let queued = as_task(fixture.admin(Action::TaskRequeue {
        task_id: running.id.clone(),
        expected_version: stopped.version,
    }));
    let new_attempt = as_task(fixture.worker(Action::TaskClaim {
        task_id: running.id.clone(),
        expected_version: queued.version,
    }));
    response(fixture.worker(Action::TaskResult {
        task_id: running.id.clone(),
        attempt_id: running.attempts[0].id.clone(),
        result: result_data(vec![]),
        expected_version: new_attempt.version,
    }));
    assert_eq!(
        as_task(fixture.admin(Action::TaskGet {
            task_id: running.id
        })),
        new_attempt
    );
    assert!(
        matches!(response(fixture.admin(Action::EventsRead { after: 0, limit: MAX_EVENTS })),
        Response::Events { events, .. } if events.iter().any(|event| event.kind == "task_late_result"))
    );
}

#[test]
fn roles_tokens_and_remote_project_creation_are_enforced() {
    let fixture = Fixture::new("roles");
    assert_error(
        fixture.store.process(
            "other",
            &request(
                None,
                Action::ProjectCreate {
                    name: "remote".into(),
                },
            ),
        ),
        "forbidden",
    );
    assert_error(
        fixture.worker(Action::TaskAccept {
            task_id: fixture.task.id.clone(),
            expected_version: 1,
        }),
        "forbidden",
    );
    assert_error(
        fixture.worker(Action::MemberSet {
            device_id: "worker".into(),
            role: Role::Admin,
            revoked: false,
        }),
        "forbidden",
    );
    let mut forged = fixture.auth.clone();
    forged.token = "wrong".into();
    assert_error(
        fixture.store.process(
            "worker",
            &task_request(
                &fixture.project,
                Action::TaskClaim {
                    task_id: fixture.task.id.clone(),
                    expected_version: 1,
                },
                Some(forged),
            ),
        ),
        "forbidden",
    );
    assert_error(
        fixture.store.process(
            "admin",
            &task_request(
                &fixture.project,
                Action::SessionHeartbeat { available: true },
                Some(fixture.auth.clone()),
            ),
        ),
        "forbidden",
    );
    response(fixture.admin(Action::MemberSet {
        device_id: "worker".into(),
        role: Role::Viewer,
        revoked: false,
    }));
    assert_error(
        fixture.worker(Action::TaskClaim {
            task_id: fixture.task.id.clone(),
            expected_version: 1,
        }),
        "forbidden",
    );
    assert_error(
        fixture.admin(Action::MemberSet {
            device_id: "admin".into(),
            role: Role::Worker,
            revoked: false,
        }),
        "invalid_state",
    );
}
