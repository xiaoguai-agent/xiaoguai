//! Turnkey expert personas (v1.34) — seed the curated expert roles once.
//!
//! `run_serve` calls [`ensure_expert_personas`] at boot so a fresh install
//! ships with ready-to-pick expert assistants. Their names MUST match the
//! `persona_name` fields in `xiaoguai-api`'s `expert_prerequisites.json`
//! (that catalog gates each persona's selectability on its prerequisites) —
//! the [`tests::seed_names_are_stable`] guard pins them so a rename can't
//! silently break the gate.
//!
//! Semantics: **create-if-never-existed**, not create-if-missing — the
//! existence check queries the `personas` table directly and counts archived
//! rows too, so an owner who archives (deletes) a seeded persona does NOT get
//! it resurrected on the next boot. Renames keep the row, so they are equally
//! safe. Non-fatal: any failure logs and boot continues.

use sqlx::SqlitePool;
use xiaoguai_personas::{CreatePersonaRequest, PersonaRepository};

/// The flagship VMware-ops persona. Contains both "`VMware`" and "运维" so the
/// chat-ui's `isVmOpsPersonaName` heuristic surfaces the starter card.
pub const VM_OPS_PERSONA_NAME: &str = "VMware 运维助手";
/// `VMware` network-ops persona (NSX / NSX-Security / AVI).
pub const VM_NETWORK_PERSONA_NAME: &str = "VMware 网络运维助手";
/// General data-analyst persona (SQL data source).
pub const DATA_ANALYST_PERSONA_NAME: &str = "数据分析助手";

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

const VM_NETWORK_SYSTEM_PROMPT: &str = "\
你是「VMware 网络运维助手」,负责 NSX / NSX 安全 / AVI 的网络运维与策略。

工作准则:
1. 只读优先:先用只读能力查拓扑、策略、分段、负载均衡状态,再谈变更。
2. 有就是有,没有就是没有:只陈述工具真实返回的数据;相关 MCP 未装或未连接就直说,
   绝不编造网络对象、规则或状态。
3. 变更三步走:改防火墙规则 / 分段 / NAT / 负载均衡前,先复述影响面(哪些工作负载、
   哪个租户)→ 征得明确同意 → 执行后回报真实结果。网络变更影响面大,逐项确认,不批量静默执行。
4. 凭据安全:NSX/AVI 凭据只在服务器端配置文件里,绝不在对话中展示或索取。";

const DATA_ANALYST_SYSTEM_PROMPT: &str = "\
你是「数据分析助手」,基于已连接的 SQL 数据库回答分析问题。

工作准则:
1. 只读优先:默认只做 SELECT / 只读查询;任何写操作(INSERT/UPDATE/DELETE/DDL)必须
   先复述并征得用户明确同意。
2. 有就是有,没有就是没有:只根据查询真实返回的行作答;没有数据源连接就直说「未连接数据库」,
   绝不编造数字、表结构或结论。
3. 先看后算:给出结论前,先展示所用的查询与样本行,让用户可核对。
4. 大结果集先聚合 / 采样再展示,避免刷屏。";

/// One seedable expert persona.
struct SeedPersona {
    name: &'static str,
    system_prompt: &'static str,
}

const SEED_PERSONAS: &[SeedPersona] = &[
    SeedPersona {
        name: VM_OPS_PERSONA_NAME,
        system_prompt: VM_OPS_SYSTEM_PROMPT,
    },
    SeedPersona {
        name: VM_NETWORK_PERSONA_NAME,
        system_prompt: VM_NETWORK_SYSTEM_PROMPT,
    },
    SeedPersona {
        name: DATA_ANALYST_PERSONA_NAME,
        system_prompt: DATA_ANALYST_SYSTEM_PROMPT,
    },
];

/// Seed each curated expert persona that has NEVER existed (archived rows
/// count — an owner-archived seed stays gone). Returns the names created this
/// boot. Each persona is independent: one failure logs and the rest proceed.
///
/// # Errors
/// Never returns `Err` — per-persona failures are logged and skipped so a seed
/// problem can't block boot. The `Result` is kept for call-site symmetry.
pub async fn ensure_expert_personas(
    pool: &SqlitePool,
    personas: &dyn PersonaRepository,
) -> anyhow::Result<Vec<String>> {
    let mut created = Vec::new();
    for seed in SEED_PERSONAS {
        match ensure_one(pool, personas, seed).await {
            Ok(true) => created.push(seed.name.to_string()),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(persona = seed.name, error = %e, "expert persona seed failed (skipping)");
            }
        }
    }
    Ok(created)
}

async fn ensure_one(
    pool: &SqlitePool,
    personas: &dyn PersonaRepository,
    seed: &SeedPersona,
) -> anyhow::Result<bool> {
    // Direct table query on purpose: `PersonaRepository::list` hides archived
    // rows, and an owner-archived seed must stay gone.
    let ever_existed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM personas WHERE name = ?")
        .bind(seed.name)
        .fetch_one(pool)
        .await?;
    if ever_existed > 0 {
        return Ok(false);
    }
    personas
        .create(&CreatePersonaRequest {
            name: seed.name.to_string(),
            system_prompt: seed.system_prompt.to_string(),
            default_model: None,
            // Unrestricted: the owner opts into narrowing via the persona's
            // `tool_allowlist` (now enforced per turn) once they know which
            // tools they run.
            tool_allowlist: None,
            escalation_tier: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("create persona {}: {e}", seed.name))?;
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

    #[test]
    fn seed_names_are_stable() {
        // These MUST equal the persona_name fields in xiaoguai-api's
        // expert_prerequisites.json, or the readiness gate stops matching.
        assert_eq!(VM_OPS_PERSONA_NAME, "VMware 运维助手");
        assert_eq!(VM_NETWORK_PERSONA_NAME, "VMware 网络运维助手");
        assert_eq!(DATA_ANALYST_PERSONA_NAME, "数据分析助手");
        assert_eq!(SEED_PERSONAS.len(), 3);
    }

    #[tokio::test]
    async fn seeds_all_experts_once_then_idempotent() {
        let pool = pool().await;
        let repo = SqlitePersonaRepository::new(pool.clone());
        let first = ensure_expert_personas(&pool, &repo).await.unwrap();
        assert_eq!(first.len(), 3);
        // Second boot: all already exist → nothing created.
        let second = ensure_expert_personas(&pool, &repo).await.unwrap();
        assert!(second.is_empty());
        let all = repo.list().await.unwrap();
        for seed in SEED_PERSONAS {
            assert_eq!(all.iter().filter(|p| p.name == seed.name).count(), 1);
        }
        // The VM-ops prompt carries the anti-hallucination rule.
        let vm = all.iter().find(|p| p.name == VM_OPS_PERSONA_NAME).unwrap();
        assert!(vm.system_prompt.contains("有就是有"));
    }

    #[tokio::test]
    async fn archived_seed_stays_gone() {
        let pool = pool().await;
        let repo = SqlitePersonaRepository::new(pool.clone());
        ensure_expert_personas(&pool, &repo).await.unwrap();
        let vm = repo
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.name == VM_OPS_PERSONA_NAME)
            .unwrap();
        repo.archive_persona(vm.id).await.unwrap();
        // Next boot: archived row still counts as "existed" — no revival, and
        // the other two are untouched.
        let created = ensure_expert_personas(&pool, &repo).await.unwrap();
        assert!(created.is_empty());
        assert!(repo
            .list()
            .await
            .unwrap()
            .iter()
            .all(|p| p.name != VM_OPS_PERSONA_NAME));
    }
}
