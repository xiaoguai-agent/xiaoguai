//! HR Onboarding Pack — integration tests
//!
//! Tests exercise the pack's logic by wiring real xiaoguai-orchestrator
//! primitives (`Supervisor`, `Budget`, `PlanStep`) with in-process mock workers and
//! planners.  No external systems (Okta, Google Workspace, Calendar, Feishu)
//! are called.
//!
//! Test groups
//! -----------
//! 1. `plan_decomposition`  — coordinator produces the 4 expected subtasks in order
//! 2. `worker_side_effects` — each worker mock records the right audit entries
//! 3. `failure_handling`    — one failing subtask does not abort the run; report flags it
//!
//! Run:
//!   cargo test -p xiaoguai-orchestrator --test `hr_onboarding_tests`
//!
//! (The test file lives under packs/hr-onboarding/tests/ but is gated by a
//! path in the orchestrator's Cargo.toml [[test]] stanza added in this tag.)

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use xiaoguai_orchestrator::{
    budget::Budget,
    plan::PlanStep,
    planner::Planner,
    supervisor::{RunOutcome, StepResult},
    worker::{Task, Worker, WorkerResult},
    OrchestratorError, Supervisor,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Canonical onboarding goal string (mirrors `coordinator.yaml` `static_plan`).
const ONBOARD_GOAL: &str =
    "Onboard new employee: name=Alice email=alice@company.com employee_id=emp-001";

/// The four canonical HR step ids in dependency order.
const STEP_IDS: [&str; 4] = [
    "account_provisioning",
    "meeting_scheduling",
    "welcome_messaging",
    "buddy_assignment",
];

/// Build the static 4-step HR onboarding plan (mirrors coordinator.yaml).
fn hr_onboarding_plan() -> Vec<PlanStep> {
    vec![
        PlanStep::new(
            "account_provisioning",
            "Create Okta account, Google Workspace account, and GitHub org membership for employee_id=emp-001 (email=alice@company.com)",
            vec![],
        ),
        PlanStep::new(
            "meeting_scheduling",
            "Schedule Day-1 orientation, manager 1:1, team intro, and IT setup call for employee_id=emp-001",
            vec!["account_provisioning".to_string()],
        ),
        PlanStep::new(
            "welcome_messaging",
            "Send welcome email and Feishu group invite to employee_id=emp-001 (email=alice@company.com)",
            vec!["account_provisioning".to_string()],
        ),
        PlanStep::new(
            "buddy_assignment",
            "Assign onboarding buddy for employee_id=emp-001 and notify buddy via Feishu DM",
            vec!["welcome_messaging".to_string()],
        ),
    ]
}

// ── Mock planner ──────────────────────────────────────────────────────────────

/// Emits the 4 HR plan steps in order; returns None once the queue is empty.
struct HrOnboardingPlanner {
    steps: Mutex<VecDeque<PlanStep>>,
    /// Tracks the goals passed to `next_step` (for assertion in tests).
    seen_goals: Mutex<Vec<String>>,
}

impl HrOnboardingPlanner {
    fn new() -> Self {
        Self {
            steps: Mutex::new(VecDeque::from(hr_onboarding_plan())),
            seen_goals: Mutex::new(Vec::new()),
        }
    }

    #[allow(dead_code, reason = "test helper — used in some test variants")]
    fn seen_goal_count(&self) -> usize {
        self.seen_goals.lock().unwrap().len()
    }
}

#[async_trait]
impl Planner for HrOnboardingPlanner {
    async fn next_step(
        &self,
        goal: &str,
        _history: &[StepResult],
    ) -> Result<Option<PlanStep>, OrchestratorError> {
        self.seen_goals.lock().unwrap().push(goal.to_string());
        Ok(self.steps.lock().unwrap().pop_front())
    }
}

// ── Mock audit sink ───────────────────────────────────────────────────────────

/// Simulates the `onboarding_audit_log` and `scheduled_meetings` tables.
/// Workers call `record_audit` / `record_meeting` instead of touching a real DB.
#[derive(Default)]
struct MockAuditSink {
    audit_entries: Mutex<Vec<AuditEntry>>,
    meeting_entries: Mutex<Vec<MeetingEntry>>,
    im_sends: Mutex<Vec<ImSend>>,
}

#[derive(Debug, Clone)]
struct AuditEntry {
    #[allow(dead_code, reason = "recorded for future assertions")]
    employee_id: String,
    #[allow(dead_code, reason = "recorded for future assertions")]
    step_id: String,
    action: String,
    success: bool,
}

#[derive(Debug, Clone)]
struct MeetingEntry {
    #[allow(dead_code, reason = "recorded for future assertions")]
    employee_id: String,
    title: String,
}

#[derive(Debug, Clone)]
struct ImSend {
    #[allow(dead_code, reason = "recorded for future assertions")]
    recipient: String,
    message_kind: String, // "welcome_dm" | "group_chat" | "buddy_notify"
}

impl MockAuditSink {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[allow(dead_code, reason = "test helper — available for assertions")]
    fn audit_count(&self) -> usize {
        self.audit_entries.lock().unwrap().len()
    }

    fn meeting_count(&self) -> usize {
        self.meeting_entries.lock().unwrap().len()
    }

    fn im_send_count(&self) -> usize {
        self.im_sends.lock().unwrap().len()
    }

    fn has_audit_action(&self, action: &str) -> bool {
        self.audit_entries
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.action == action)
    }

    fn has_meeting(&self, title: &str) -> bool {
        self.meeting_entries
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.title == title)
    }

    fn has_im_send(&self, kind: &str) -> bool {
        self.im_sends
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.message_kind == kind)
    }

    fn failed_audit_count(&self) -> usize {
        self.audit_entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| !e.success)
            .count()
    }
}

// ── Mock workers ──────────────────────────────────────────────────────────────

/// Mock for the account-provisioner agent.
/// Side effect: 3 audit entries (`okta`, `google_workspace`, `github`).
struct MockAccountProvisioner {
    sink: Arc<MockAuditSink>,
    /// If Some(msg), the worker fails with that message.
    fail_with: Option<String>,
}

impl MockAccountProvisioner {
    fn ok(sink: Arc<MockAuditSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            fail_with: None,
        })
    }

    fn failing(sink: Arc<MockAuditSink>, msg: &str) -> Arc<Self> {
        Arc::new(Self {
            sink,
            fail_with: Some(msg.to_string()),
        })
    }
}

#[async_trait]
impl Worker for MockAccountProvisioner {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let employee_id = extract_field(&task.description, "employee_id");

        if let Some(ref msg) = self.fail_with {
            self.sink.audit_entries.lock().unwrap().push(AuditEntry {
                employee_id: employee_id.unwrap_or_default(),
                step_id: task.step_id.clone(),
                action: "account_created".to_string(),
                success: false,
            });
            return Ok(WorkerResult {
                output: msg.clone(),
                success: false,
            });
        }

        for system in ["okta", "google_workspace", "github"] {
            self.sink.audit_entries.lock().unwrap().push(AuditEntry {
                employee_id: employee_id.clone().unwrap_or_default(),
                step_id: task.step_id.clone(),
                action: format!("account_created:{system}"),
                success: true,
            });
        }

        Ok(WorkerResult {
            output: "Accounts created: okta, google_workspace, github".to_string(),
            success: true,
        })
    }
}

/// Mock for the meeting-scheduler agent.
/// Side effect: 4 meeting entries.
struct MockMeetingScheduler {
    sink: Arc<MockAuditSink>,
    fail_with: Option<String>,
}

impl MockMeetingScheduler {
    fn ok(sink: Arc<MockAuditSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            fail_with: None,
        })
    }

    fn failing(sink: Arc<MockAuditSink>, msg: &str) -> Arc<Self> {
        Arc::new(Self {
            sink,
            fail_with: Some(msg.to_string()),
        })
    }
}

#[async_trait]
impl Worker for MockMeetingScheduler {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let employee_id = extract_field(&task.description, "employee_id");

        if let Some(ref msg) = self.fail_with {
            self.sink.audit_entries.lock().unwrap().push(AuditEntry {
                employee_id: employee_id.unwrap_or_default(),
                step_id: task.step_id.clone(),
                action: "meeting_scheduled".to_string(),
                success: false,
            });
            return Ok(WorkerResult {
                output: msg.clone(),
                success: false,
            });
        }

        let eid = employee_id.unwrap_or_default();
        for title in [
            "Day-1 Orientation",
            "Manager 1:1",
            "Team Introduction",
            "IT Setup",
        ] {
            self.sink
                .meeting_entries
                .lock()
                .unwrap()
                .push(MeetingEntry {
                    employee_id: eid.clone(),
                    title: title.to_string(),
                });
        }

        Ok(WorkerResult {
            output:
                "Meetings scheduled: Day-1 Orientation, Manager 1:1, Team Introduction, IT Setup"
                    .to_string(),
            success: true,
        })
    }
}

/// Mock for the welcome-messenger agent.
/// Side effect: 2 IM sends (`welcome_dm` + `group_chat`).
struct MockWelcomeMessenger {
    sink: Arc<MockAuditSink>,
    fail_with: Option<String>,
}

impl MockWelcomeMessenger {
    fn ok(sink: Arc<MockAuditSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            fail_with: None,
        })
    }

    #[allow(dead_code, reason = "test helper — used in failure scenario tests")]
    fn failing(sink: Arc<MockAuditSink>, msg: &str) -> Arc<Self> {
        Arc::new(Self {
            sink,
            fail_with: Some(msg.to_string()),
        })
    }
}

#[async_trait]
impl Worker for MockWelcomeMessenger {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let email = extract_field(&task.description, "email");

        if let Some(ref msg) = self.fail_with {
            self.sink.audit_entries.lock().unwrap().push(AuditEntry {
                employee_id: extract_field(&task.description, "employee_id").unwrap_or_default(),
                step_id: task.step_id.clone(),
                action: "welcome_message_sent".to_string(),
                success: false,
            });
            return Ok(WorkerResult {
                output: msg.clone(),
                success: false,
            });
        }

        let recipient = email.unwrap_or_else(|| "unknown".to_string());
        self.sink.im_sends.lock().unwrap().push(ImSend {
            recipient: recipient.clone(),
            message_kind: "welcome_dm".to_string(),
        });
        self.sink.im_sends.lock().unwrap().push(ImSend {
            recipient,
            message_kind: "group_chat".to_string(),
        });

        Ok(WorkerResult {
            output: "Welcome DM sent; group chat 'Onboarding: Alice' created".to_string(),
            success: true,
        })
    }
}

/// Mock for the buddy-assigner agent.
/// Side effect: 1 audit entry (`buddy_assigned`) + 1 IM send (`buddy_notify`).
struct MockBuddyAssigner {
    sink: Arc<MockAuditSink>,
    /// Simulated buddy from the round-robin pool.
    buddy_email: String,
}

impl MockBuddyAssigner {
    fn ok(sink: Arc<MockAuditSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            buddy_email: "alice.chen@company.com".to_string(),
        })
    }
}

#[async_trait]
impl Worker for MockBuddyAssigner {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let employee_id = extract_field(&task.description, "employee_id").unwrap_or_default();

        self.sink.audit_entries.lock().unwrap().push(AuditEntry {
            employee_id: employee_id.clone(),
            step_id: task.step_id.clone(),
            action: "buddy_assigned".to_string(),
            success: true,
        });

        self.sink.im_sends.lock().unwrap().push(ImSend {
            recipient: self.buddy_email.clone(),
            message_kind: "buddy_notify".to_string(),
        });

        Ok(WorkerResult {
            output: format!("Buddy assigned: {} notified", self.buddy_email),
            success: true,
        })
    }
}

// ── Routing worker: dispatch by step_id prefix ────────────────────────────────

/// Wraps four specialist workers and routes by the `step_id` prefix,
/// mirroring the `coordinator.yaml` `handles_step_prefix` configuration.
struct RoutingWorker {
    account_provisioner: Arc<dyn Worker>,
    meeting_scheduler: Arc<dyn Worker>,
    welcome_messenger: Arc<dyn Worker>,
    buddy_assigner: Arc<dyn Worker>,
}

#[async_trait]
impl Worker for RoutingWorker {
    async fn execute(&self, task: Task) -> Result<WorkerResult, OrchestratorError> {
        let worker: Arc<dyn Worker> = if task.step_id.starts_with("account") {
            self.account_provisioner.clone()
        } else if task.step_id.starts_with("meeting") {
            self.meeting_scheduler.clone()
        } else if task.step_id.starts_with("welcome") {
            self.welcome_messenger.clone()
        } else if task.step_id.starts_with("buddy") {
            self.buddy_assigner.clone()
        } else {
            return Err(OrchestratorError::Internal(format!(
                "unknown step_id prefix: {}",
                task.step_id
            )));
        };
        worker.execute(task).await
    }
}

// ── Utility ───────────────────────────────────────────────────────────────────

/// Extract `key=<value>` from a description string.
fn extract_field(description: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    description.split_whitespace().find_map(|token| {
        token.strip_prefix(&prefix).map(|v| {
            v.trim_end_matches(|c: char| {
                !c.is_alphanumeric() && c != '@' && c != '-' && c != '_' && c != '.'
            })
            .to_string()
        })
    })
}

/// Build a standard budget suitable for HR onboarding (10 steps max).
fn standard_budget() -> Budget {
    Budget::new().with_max_steps(10)
}

/// Build a complete Supervisor wired with the routing worker.
fn build_supervisor(sink: &Arc<MockAuditSink>) -> Supervisor {
    let planner = HrOnboardingPlanner::new();
    let budget = standard_budget();
    let mut supervisor = Supervisor::new(budget, Box::new(planner));

    let router = Arc::new(RoutingWorker {
        account_provisioner: MockAccountProvisioner::ok(Arc::clone(sink)),
        meeting_scheduler: MockMeetingScheduler::ok(Arc::clone(sink)),
        welcome_messenger: MockWelcomeMessenger::ok(Arc::clone(sink)),
        buddy_assigner: MockBuddyAssigner::ok(Arc::clone(sink)),
    });
    supervisor.add_worker(router);
    supervisor
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test group 1: Plan decomposition
// ═══════════════════════════════════════════════════════════════════════════════

/// The planner emits exactly the 4 onboarding steps and the supervisor
/// reaches `GoalAchieved` after dispatching all of them.
#[tokio::test]
async fn plan_produces_four_subtasks() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    assert_eq!(
        report.history.len(),
        4,
        "expected exactly 4 steps dispatched"
    );
}

/// The four steps are dispatched in the correct dependency order.
/// `account_provisioning` must precede `meeting_scheduling`, `welcome_messaging`,
/// and `buddy_assignment`; `buddy_assignment` must come after `welcome_messaging`.
#[tokio::test]
async fn subtasks_dispatched_in_correct_order() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    let step_ids: Vec<&str> = report.history.iter().map(|s| s.step_id.as_str()).collect();

    // account_provisioning is always first
    assert_eq!(
        step_ids[0], STEP_IDS[0],
        "account_provisioning must be first"
    );

    // meeting_scheduling and welcome_messaging follow account_provisioning
    let account_pos = 0usize;
    let meeting_pos = step_ids
        .iter()
        .position(|&id| id == "meeting_scheduling")
        .expect("meeting_scheduling in history");
    let welcome_pos = step_ids
        .iter()
        .position(|&id| id == "welcome_messaging")
        .expect("welcome_messaging in history");
    let buddy_pos = step_ids
        .iter()
        .position(|&id| id == "buddy_assignment")
        .expect("buddy_assignment in history");

    assert!(
        meeting_pos > account_pos,
        "meeting_scheduling must come after account_provisioning"
    );
    assert!(
        welcome_pos > account_pos,
        "welcome_messaging must come after account_provisioning"
    );
    assert!(
        buddy_pos > welcome_pos,
        "buddy_assignment must come after welcome_messaging"
    );
}

/// All four canonical step ids appear in the dispatched plan.
#[tokio::test]
async fn all_four_canonical_step_ids_dispatched() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    for expected_id in STEP_IDS {
        assert!(
            report.history.iter().any(|s| s.step_id == expected_id),
            "step_id '{expected_id}' missing from dispatch history"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test group 2: Worker side effects
// ═══════════════════════════════════════════════════════════════════════════════

/// Account provisioner writes 3 audit entries (one per system).
#[tokio::test]
async fn account_provisioner_writes_three_audit_entries() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);
    supervisor.run(ONBOARD_GOAL).await.expect("run ok");

    // 3 from account provisioner + 1 from buddy assigner = 4 total
    // Check the specific account_created actions
    assert!(
        sink.has_audit_action("account_created:okta"),
        "okta entry missing"
    );
    assert!(
        sink.has_audit_action("account_created:google_workspace"),
        "google_workspace entry missing"
    );
    assert!(
        sink.has_audit_action("account_created:github"),
        "github entry missing"
    );
}

/// Meeting scheduler writes 4 meeting entries to the mock meetings table.
#[tokio::test]
async fn meeting_scheduler_writes_four_meetings() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);
    supervisor.run(ONBOARD_GOAL).await.expect("run ok");

    assert_eq!(sink.meeting_count(), 4, "expected 4 scheduled meetings");
    assert!(sink.has_meeting("Day-1 Orientation"));
    assert!(sink.has_meeting("Manager 1:1"));
    assert!(sink.has_meeting("Team Introduction"));
    assert!(sink.has_meeting("IT Setup"));
}

/// Welcome messenger sends 2 IM messages (`welcome_dm` + `group_chat`).
#[tokio::test]
async fn welcome_messenger_sends_two_im_messages() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);
    supervisor.run(ONBOARD_GOAL).await.expect("run ok");

    assert!(sink.has_im_send("welcome_dm"), "welcome DM missing");
    assert!(
        sink.has_im_send("group_chat"),
        "group chat creation missing"
    );
}

/// Buddy assigner writes one audit entry and sends one IM notification.
#[tokio::test]
async fn buddy_assigner_notifies_buddy_via_im() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);
    supervisor.run(ONBOARD_GOAL).await.expect("run ok");

    assert!(
        sink.has_audit_action("buddy_assigned"),
        "buddy_assigned audit entry missing"
    );
    assert!(
        sink.has_im_send("buddy_notify"),
        "buddy notification IM missing"
    );
}

/// Total IM sends across the run: 2 (`welcome_dm` + `group_chat`) + 1 (`buddy_notify`) = 3.
#[tokio::test]
async fn total_im_sends_across_run_is_three() {
    let sink = MockAuditSink::new();
    let mut supervisor = build_supervisor(&sink);
    supervisor.run(ONBOARD_GOAL).await.expect("run ok");

    assert_eq!(sink.im_send_count(), 3, "expected 3 IM sends total");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test group 3: Failure handling
// ═══════════════════════════════════════════════════════════════════════════════

/// If the meeting-scheduler fails, the supervisor continues and the other
/// steps (`welcome_messaging`, `buddy_assignment`) still complete.
#[tokio::test]
async fn meeting_scheduler_failure_does_not_abort_run() {
    let sink = MockAuditSink::new();
    let planner = HrOnboardingPlanner::new();
    let budget = standard_budget();
    let mut supervisor = Supervisor::new(budget, Box::new(planner));

    let router = Arc::new(RoutingWorker {
        account_provisioner: MockAccountProvisioner::ok(sink.clone()),
        // Meeting scheduler fails
        meeting_scheduler: MockMeetingScheduler::failing(
            sink.clone(),
            "Google Calendar: calendar conflict at 09:00",
        ),
        welcome_messenger: MockWelcomeMessenger::ok(sink.clone()),
        buddy_assigner: MockBuddyAssigner::ok(sink.clone()),
    });
    supervisor.add_worker(router);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    // Run still achieves goal (planner exhausted all steps)
    assert_eq!(report.outcome, RunOutcome::GoalAchieved);
    assert_eq!(report.history.len(), 4, "all 4 steps attempted");

    // meeting_scheduling step is flagged as failed
    let meeting_step = report
        .history
        .iter()
        .find(|s| s.step_id == "meeting_scheduling")
        .expect("meeting_scheduling in history");
    assert!(
        !meeting_step.success,
        "meeting step should be flagged failed"
    );
    assert!(
        meeting_step.output.contains("calendar conflict"),
        "failure output should contain error detail"
    );

    // welcome_messaging and buddy_assignment still succeeded
    let welcome_step = report
        .history
        .iter()
        .find(|s| s.step_id == "welcome_messaging")
        .expect("welcome_messaging in history");
    assert!(
        welcome_step.success,
        "welcome step should succeed despite meeting failure"
    );

    let buddy_step = report
        .history
        .iter()
        .find(|s| s.step_id == "buddy_assignment")
        .expect("buddy_assignment in history");
    assert!(buddy_step.success, "buddy step should succeed");
}

/// If the account-provisioner fails, all steps still run (the Supervisor's
/// default policy is continue-on-failure, not abort).
#[tokio::test]
async fn account_provisioner_failure_all_steps_still_attempted() {
    let sink = MockAuditSink::new();
    let planner = HrOnboardingPlanner::new();
    let budget = standard_budget();
    let mut supervisor = Supervisor::new(budget, Box::new(planner));

    let router = Arc::new(RoutingWorker {
        account_provisioner: MockAccountProvisioner::failing(
            sink.clone(),
            "Okta API: email already exists",
        ),
        meeting_scheduler: MockMeetingScheduler::ok(sink.clone()),
        welcome_messenger: MockWelcomeMessenger::ok(sink.clone()),
        buddy_assigner: MockBuddyAssigner::ok(sink.clone()),
    });
    supervisor.add_worker(router);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    // All 4 steps attempted
    assert_eq!(report.history.len(), 4);

    // account_provisioning failed
    let acct = report
        .history
        .iter()
        .find(|s| s.step_id == "account_provisioning")
        .unwrap();
    assert!(!acct.success);

    // remaining steps still ran and succeeded
    for step_id in [
        "meeting_scheduling",
        "welcome_messaging",
        "buddy_assignment",
    ] {
        let step = report
            .history
            .iter()
            .find(|s| s.step_id == step_id)
            .unwrap_or_else(|| panic!("{step_id} missing from history"));
        assert!(
            step.success,
            "{step_id} should succeed even though account_provisioning failed"
        );
    }
}

/// When a step fails, the failure is recorded in the run history so the
/// coordinator can flag it in the onboarding report.
#[tokio::test]
async fn failure_is_visible_in_run_history() {
    let sink = MockAuditSink::new();
    let planner = HrOnboardingPlanner::new();
    let budget = standard_budget();
    let mut supervisor = Supervisor::new(budget, Box::new(planner));

    let router = Arc::new(RoutingWorker {
        account_provisioner: MockAccountProvisioner::ok(sink.clone()),
        meeting_scheduler: MockMeetingScheduler::failing(sink.clone(), "meeting scheduler error"),
        welcome_messenger: MockWelcomeMessenger::ok(sink.clone()),
        buddy_assigner: MockBuddyAssigner::ok(sink.clone()),
    });
    supervisor.add_worker(router);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    let failed_steps: Vec<_> = report.history.iter().filter(|s| !s.success).collect();
    assert_eq!(failed_steps.len(), 1, "exactly one step should fail");
    assert_eq!(failed_steps[0].step_id, "meeting_scheduling");
}

/// Audit entries are written even for failed steps (for manual follow-up).
#[tokio::test]
async fn failed_step_still_writes_audit_entry() {
    let sink = MockAuditSink::new();
    let planner = HrOnboardingPlanner::new();
    let budget = standard_budget();
    let mut supervisor = Supervisor::new(budget, Box::new(planner));

    let router = Arc::new(RoutingWorker {
        account_provisioner: MockAccountProvisioner::ok(sink.clone()),
        meeting_scheduler: MockMeetingScheduler::failing(sink.clone(), "calendar failure"),
        welcome_messenger: MockWelcomeMessenger::ok(sink.clone()),
        buddy_assigner: MockBuddyAssigner::ok(sink.clone()),
    });
    supervisor.add_worker(router);

    supervisor.run(ONBOARD_GOAL).await.expect("run ok");

    // The meeting scheduler's on_failure: audit: true writes a failed audit entry
    assert!(
        sink.failed_audit_count() > 0,
        "failed step should write audit entry"
    );
}

/// A budget of 2 steps stops after 2 dispatches (`BudgetExhausted`), even
/// though 4 steps are planned.
#[tokio::test]
async fn budget_exhaustion_stops_after_two_steps() {
    let sink = MockAuditSink::new();
    let planner = HrOnboardingPlanner::new();
    let tight_budget = Budget::new().with_max_steps(2);
    let mut supervisor = Supervisor::new(tight_budget, Box::new(planner));

    let router = Arc::new(RoutingWorker {
        account_provisioner: MockAccountProvisioner::ok(sink.clone()),
        meeting_scheduler: MockMeetingScheduler::ok(sink.clone()),
        welcome_messenger: MockWelcomeMessenger::ok(sink.clone()),
        buddy_assigner: MockBuddyAssigner::ok(sink.clone()),
    });
    supervisor.add_worker(router);

    let report = supervisor.run_detailed(ONBOARD_GOAL).await.expect("run ok");

    assert_eq!(report.outcome, RunOutcome::BudgetExhausted);
    assert_eq!(report.history.len(), 2, "only 2 steps should have run");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test group 4: Utility / extract_field helper
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn extract_field_finds_employee_id() {
    let desc = "Create accounts for employee_id=emp-001 (email=alice@company.com)";
    assert_eq!(
        extract_field(desc, "employee_id"),
        Some("emp-001".to_string())
    );
}

#[test]
fn extract_field_finds_email() {
    let desc = "Send welcome to employee_id=emp-001 email=alice@company.com end";
    assert_eq!(
        extract_field(desc, "email"),
        Some("alice@company.com".to_string())
    );
}

#[test]
fn extract_field_returns_none_for_missing_key() {
    let desc = "No relevant fields here";
    assert_eq!(extract_field(desc, "employee_id"), None);
}
