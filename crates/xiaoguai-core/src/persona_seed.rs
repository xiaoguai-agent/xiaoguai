//! Turnkey VM-ops persona (v1.34) — seed 「`VMware` 运维助手」 exactly once.
//!
//! `run_serve` calls [`ensure_vm_ops_persona`] at boot so a fresh install has
//! a ready-to-pick VM-ops assistant (its name contains both "`VMware`" and
//! "运维", which is what the chat-ui keys the `VMware` starter card on).
//!
//! Semantics: **create-if-never-existed**, not create-if-missing — the
//! existence check queries the `personas` table directly and counts archived
//! rows too, so an owner who archives (deletes) the seeded persona does NOT
//! get it resurrected on the next boot. Renames keep the row, so they are
//! equally safe. Non-fatal: any failure logs and boot continues.

use sqlx::SqlitePool;
use xiaoguai_personas::{CreatePersonaRequest, PersonaRepository};

/// Display name of the seeded persona. Contains both "`VMware`" and "运维" so
/// the chat-ui's `isVmOpsPersonaName` heuristic surfaces the starter card.
pub const VM_OPS_PERSONA_NAME: &str = "VMware 运维助手";

/// Role prompt — the zw008 VMware-family safety rules distilled for chat
/// injection (只读优先 / 有就是有 / 变更三步走 / 凭据安全).
const VM_OPS_SYSTEM_PROMPT: &str = "\
你是「VMware 运维助手」,负责 vCenter / ESXi 的日常运维。

工作准则:
1. 只读优先:排查、巡检、查询一律先用只读工具(vmware-monitor / vmware-debug /
   vmware-log-insight);只有在用户明确要求变更时才使用写操作工具。
2. 有就是有,没有就是没有:只陈述工具真实返回的数据。工具不可用或未连接 vCenter 时,
   直接说明缺什么(哪个 vmware MCP server 未安装 / 未配置 `~/.vmware-*/config.yaml`),
   绝不编造清单、状态、指标或文件。
3. 变更三步走:先复述目标对象与影响范围 → 征得用户明确同意 → 执行后报告工具返回的
   真实结果。删除 / 关机 / 删快照 / 迁移等破坏性操作必须逐项确认,绝不批量静默执行。
4. 凭据安全:vCenter 密码只存在于服务器端 `~/.vmware-*/.env`。绝不在对话中展示、
   记录密码,也绝不要求用户粘贴任何密码。
5. 操作生产环境前,先提醒用户当前连接的是哪个 target(环境名),再继续。";

/// Seed the VM-ops persona if no persona with that name has EVER existed
/// (archived rows count as "existed"). Returns `Ok(true)` when a persona was
/// created this boot, `Ok(false)` when one already exists / existed.
///
/// # Errors
/// Propagates the existence query or the create call; the caller treats both
/// as non-fatal (log + continue boot).
pub async fn ensure_vm_ops_persona(
    pool: &SqlitePool,
    personas: &dyn PersonaRepository,
) -> anyhow::Result<bool> {
    // Direct table query on purpose: `PersonaRepository::list` hides archived
    // rows, and an owner-archived seed must stay gone.
    let ever_existed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personas WHERE name = ?")
        .bind(VM_OPS_PERSONA_NAME)
        .fetch_one(pool)
        .await?;
    if ever_existed > 0 {
        return Ok(false);
    }
    personas
        .create(&CreatePersonaRequest {
            name: VM_OPS_PERSONA_NAME.to_string(),
            system_prompt: VM_OPS_SYSTEM_PROMPT.to_string(),
            default_model: None,
            // Unrestricted: the owner opts into narrowing via the persona's
            // `tool_allowlist` (now enforced per turn) once they know which
            // vmware tools they run.
            tool_allowlist: None,
            escalation_tier: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("create persona {VM_OPS_PERSONA_NAME}: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xiaoguai_personas::SqlitePersonaRepository;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        xiaoguai_storage::db::migrate(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn seeds_once_then_idempotent() {
        let pool = pool().await;
        let repo = SqlitePersonaRepository::new(pool.clone());
        assert!(ensure_vm_ops_persona(&pool, &repo).await.unwrap());
        // Second boot: already exists → no duplicate.
        assert!(!ensure_vm_ops_persona(&pool, &repo).await.unwrap());
        let all = repo.list().await.unwrap();
        assert_eq!(
            all.iter().filter(|p| p.name == VM_OPS_PERSONA_NAME).count(),
            1
        );
        // The seeded prompt carries the anti-hallucination rule.
        let seeded = all.iter().find(|p| p.name == VM_OPS_PERSONA_NAME).unwrap();
        assert!(seeded.system_prompt.contains("有就是有"));
        assert!(seeded.tool_allowlist.is_none());
    }

    #[tokio::test]
    async fn archived_seed_stays_gone() {
        let pool = pool().await;
        let repo = SqlitePersonaRepository::new(pool.clone());
        assert!(ensure_vm_ops_persona(&pool, &repo).await.unwrap());
        let seeded = repo
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.name == VM_OPS_PERSONA_NAME)
            .unwrap();
        repo.archive_persona(seeded.id).await.unwrap();
        // Next boot: the archived row still counts as "existed" — no revival.
        assert!(!ensure_vm_ops_persona(&pool, &repo).await.unwrap());
        assert!(repo
            .list()
            .await
            .unwrap()
            .iter()
            .all(|p| p.name != VM_OPS_PERSONA_NAME));
    }
}
