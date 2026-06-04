//! E2E test for the agent-authored-skills lifecycle (Tier-2 D.1).
//!
//! Models the full path:
//!   1. Agent proposes a skill with an over-broad `tool_allowlist`.
//!   2. The `HotL` gate (mocked) DENIES the first proposal.
//!   3. Agent re-issues a narrower proposal.
//!   4. The gate ALLOWS the second proposal — row persisted as `pending`.
//!   5. Admin invokes `approve_proposal` — row flips to `installed`,
//!      manifest written as YAML to the tempdir.
//!   6. Audit chain shows the expected actions in order.
//!
//! Unlike a true `MockBackend` agent loop this test calls `propose` /
//! `approve_proposal` directly — the agent-side `propose_skill` tool
//! plumbing is exercised by `xiaoguai-agent`'s own tests, and an
//! end-to-end ReAct loop test belongs there (it would otherwise drag
//! `xiaoguai-llm` into `xiaoguai-tasks`'s dev-dependencies). The flow
//! observable through `propose` is identical to what the tool wrapper
//! produces.

use std::collections::HashSet;

use xiaoguai_tasks::skill_author::{
    approve_proposal, propose, AllowAllSkillGate, DenyThenAllowGate, InMemoryAuditSink,
    InMemorySkillProposalRepository, InMemoryTenantSettings, ProposalStatus, SkillAuthorCtx,
    SkillAuthorError, SkillManifest, SkillProposalRepository,
};

fn known_tools() -> HashSet<String> {
    ["search", "fetch_url", "summarise"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn first_draft() -> SkillManifest {
    // Over-broad: references a tool the toolbox does not expose
    // ("send_email") — should fail validation BEFORE the gate.
    SkillManifest {
        name: "ar-collector".into(),
        description: "Collect overdue AR invoices".into(),
        version: "0.1.0".into(),
        system_prompt: "You collect AR invoices via email and web search.".into(),
        tool_allowlist: vec!["search".into(), "send_email".into()],
    }
}

fn revised_draft() -> SkillManifest {
    // Narrower — only references registered tools.
    SkillManifest {
        name: "ar-collector".into(),
        description: "Collect overdue AR invoices".into(),
        version: "0.1.0".into(),
        system_prompt: "You collect AR invoices using search and summarisation.".into(),
        tool_allowlist: vec!["search".into(), "summarise".into()],
    }
}

#[tokio::test]
async fn agent_proposes_then_admin_approves_full_lifecycle() {
    // ── Setup ──────────────────────────────────────────────────────────────
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = InMemorySkillProposalRepository::new();
    let settings = InMemoryTenantSettings::new();
    settings.allow();
    let audit = InMemoryAuditSink::new();
    let known = known_tools();

    // First call denied at the gate (budget mock), second call allowed.
    // Used in step 3 below after the validator-rejected first draft.
    let gate = DenyThenAllowGate::new(1, "skill_author daily budget exceeded");

    let ctx = SkillAuthorCtx {
        repo: &*repo,
        settings: &*settings,
        gate: &gate,
        audit: &*audit,
        known_tools: &known,
    };

    // ── Step 1: Agent emits the over-broad draft ──────────────────────────
    // Validator rejects this BEFORE consulting the gate. No audit emission.
    let err = propose(&ctx, "agent-37", first_draft())
        .await
        .expect_err("first draft should be rejected by the validator");
    assert!(
        matches!(err, SkillAuthorError::InvalidManifest(_)),
        "expected InvalidManifest, got {err:?}"
    );
    assert!(
        audit.entries().is_empty(),
        "validator-rejected drafts must not pollute the audit log"
    );
    assert!(
        repo.list(None).await.unwrap().is_empty(),
        "no DB row should land for a validator-rejected draft"
    );

    // ── Step 2: Agent re-issues a narrower draft; gate DENIES (budget) ────
    let err = propose(&ctx, "agent-37", revised_draft())
        .await
        .expect_err("first allowed-through-validator draft is denied at the gate");
    assert!(
        matches!(err, SkillAuthorError::Denied(ref r) if r.contains("budget")),
        "expected gate denial, got {err:?}"
    );
    // Two audit rows: propose + hotl_gate (deny). No row in DB.
    let actions: Vec<_> = audit.entries().iter().map(|e| e.action.clone()).collect();
    assert_eq!(actions, vec!["skill.propose", "skill.hotl_gate"]);
    assert!(
        repo.list(None).await.unwrap().is_empty(),
        "gate-denied drafts must not be persisted"
    );

    // ── Step 3: Agent retries; gate ALLOWS ────────────────────────────────
    let row = propose(&ctx, "agent-37", revised_draft())
        .await
        .expect("second attempt should be allowed");
    assert_eq!(row.status, ProposalStatus::Pending);
    assert_eq!(row.proposed_by, "agent-37");
    assert_eq!(row.manifest.name, "ar-collector");

    // Four audit rows now: prev two + propose + hotl_gate (allow).
    let actions: Vec<_> = audit.entries().iter().map(|e| e.action.clone()).collect();
    assert_eq!(
        actions,
        vec![
            "skill.propose",
            "skill.hotl_gate",
            "skill.propose",
            "skill.hotl_gate",
        ]
    );
    // Pending visible in the list view.
    let pending = repo.list(Some(ProposalStatus::Pending)).await.unwrap();
    assert_eq!(pending.len(), 1);

    // ── Step 4: Admin approves ───────────────────────────────────────────
    let installed = approve_proposal(&ctx, &row.id, "admin-1", tmp.path())
        .await
        .expect("approve should succeed");
    assert_eq!(installed.status, ProposalStatus::Installed);
    assert_eq!(installed.decided_by.as_deref(), Some("admin-1"));
    assert!(installed.decided_at.is_some());

    // YAML on disk and round-trip parses to the proposed manifest.
    let yaml_path = tmp.path().join("ar-collector-0.1.0.yaml");
    assert!(
        yaml_path.exists(),
        "approval should write YAML to skills_dir"
    );
    let parsed: SkillManifest =
        serde_yaml::from_str(&std::fs::read_to_string(&yaml_path).unwrap()).unwrap();
    assert_eq!(parsed, revised_draft());

    // ── Step 5: Final audit chain ─────────────────────────────────────────
    let actions: Vec<_> = audit.entries().iter().map(|e| e.action.clone()).collect();
    assert_eq!(
        actions,
        vec![
            "skill.propose",
            "skill.hotl_gate",
            "skill.propose",
            "skill.hotl_gate",
            "skill.approve",
        ]
    );

    // Pending list is now empty, installed list shows one row.
    assert!(repo
        .list(Some(ProposalStatus::Pending))
        .await
        .unwrap()
        .is_empty());
    let installed_list = repo.list(Some(ProposalStatus::Installed)).await.unwrap();
    assert_eq!(installed_list.len(), 1);
}

#[tokio::test]
async fn disabled_owner_silently_drops_the_proposal() {
    let repo = InMemorySkillProposalRepository::new();
    let settings = InMemoryTenantSettings::new(); // not allowed
    let audit = InMemoryAuditSink::new();
    let known = known_tools();
    let gate = AllowAllSkillGate;
    let ctx = SkillAuthorCtx {
        repo: &*repo,
        settings: &*settings,
        gate: &gate,
        audit: &*audit,
        known_tools: &known,
    };

    let err = propose(&ctx, "agent-37", revised_draft())
        .await
        .expect_err("disabled owner should bounce");
    assert!(matches!(err, SkillAuthorError::Disabled));
    assert!(
        audit.entries().is_empty(),
        "disabled drops should not emit audit rows"
    );
    assert!(repo.list(None).await.unwrap().is_empty());
}
