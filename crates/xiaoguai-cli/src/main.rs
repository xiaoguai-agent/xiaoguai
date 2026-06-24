//! Xiaoguai CLI entry point. Subcommand bodies live in `xiaoguai_cli::commands`
//! so they remain unit-testable.

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use xiaoguai_cli::commands::{
    anomaly, audit_bundle, audit_export, backup, chat, cli_config, code, completions, doctor, eval,
    hotl, init, manpages, mcp, memory, outcomes, pack, provider, r#loop, remote, repl, schedule,
    self_update, service, skills, stats, style, tasks, watch,
};
use xiaoguai_config::Settings;
use xiaoguai_storage::{
    connect, migrate,
    repositories::{
        LlmProviderRepository, McpServerRepository, SqliteLlmProviderRepository,
        SqliteMcpServerRepository,
    },
};

mod cli_args;
#[allow(clippy::wildcard_imports)]
use cli_args::*;

fn load_settings(config: Option<&str>) -> Result<Settings> {
    match config {
        Some(path) => {
            Settings::load_from_file(path).map_err(|e| anyhow::anyhow!("load config: {e}"))
        }
        None => Settings::load_from_env().map_err(|e| anyhow::anyhow!("load env config: {e}")),
    }
}

async fn build_provider_repo(config: Option<&str>) -> Result<SqliteLlmProviderRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    // Apply migrations (schema + seeded providers) so `xiaoguai init` and the
    // `provider` commands work on a brand-new DB without needing `xiaoguai
    // serve` first. Idempotent — a no-op once the store is current.
    migrate(&pool).await.context("apply migrations")?;
    Ok(SqliteLlmProviderRepository::from_env(pool)?)
}

/// Build the local memory store the way `serve` does: same pool, same
/// migrate-on-connect, same embedder selection (`memory.embedder` config
/// block with the `OLLAMA_HOST` env override) via
/// `xiaoguai_core::memory_bridge::build_memory_store`.
async fn build_memory_store(
    config: Option<&str>,
) -> Result<std::sync::Arc<dyn xiaoguai_memory::MemoryStore>> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    migrate(&pool).await.context("apply migrations")?;
    Ok(xiaoguai_core::memory_bridge::build_memory_store(
        pool,
        &settings.memory.embedder,
    ))
}

async fn handle_memory(config: Option<&str>, action: MemoryCmd) -> Result<()> {
    let store = build_memory_store(config).await?;
    match action {
        MemoryCmd::Export { kind, out } => {
            let jsonl = memory::export(store.as_ref(), kind.as_deref()).await?;
            match out {
                Some(path) => {
                    std::fs::write(&path, &jsonl)
                        .with_context(|| format!("write export to {path}"))?;
                    println!("exported {} memorie(s) to {path}", jsonl.lines().count());
                }
                None => print!("{jsonl}"),
            }
            Ok(())
        }
        MemoryCmd::Import { file } => {
            let content = std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?;
            let report = memory::import(store.as_ref(), &content).await?;
            print!("{}", memory::format_import_report(&report));
            Ok(())
        }
    }
}

async fn build_mcp_repo(config: Option<&str>) -> Result<SqliteMcpServerRepository> {
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    Ok(SqliteMcpServerRepository::new(pool))
}

async fn handle_stats(
    config: Option<&str>,
    by: String,
    since: Option<String>,
    until: Option<String>,
    json: bool,
) -> Result<()> {
    let group_by = stats::GroupBy::parse(&by)?;
    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    let rows = stats::query(
        &pool,
        &stats::StatsArgs {
            by: group_by,
            since,
            until,
        },
    )
    .await?;
    if rows.is_empty() {
        println!("no usage recorded yet");
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&stats::to_json(&rows))?);
    } else {
        print!("{}", stats::format_table(&rows, group_by));
    }
    Ok(())
}

async fn handle_chat(
    prompt: String,
    server: String,
    user_id: String,
    mock: bool,
    ollama_url: Option<String>,
    model: String,
) -> Result<()> {
    // Direct-backend mode: `--mock` / `--ollama-url` bypass the server entirely
    // (offline / dev). Empty `--model` falls back to the Ollama default here.
    if mock || ollama_url.is_some() {
        let answer = chat::run(chat::ChatArgs {
            prompt,
            mock,
            ollama_url,
            model: if model.is_empty() {
                "qwen2.5-coder".to_string()
            } else {
                model
            },
        })
        .await?;
        println!("{answer}");
        return Ok(());
    }

    // Default: one-shot against the running server — same wire path as
    // `remote chat`, but auto-creates the session so there's no id to juggle.
    let client = remote::RemoteClient::new(server.clone());
    client.healthz().await.with_context(|| {
        format!(
            "could not reach the server at {server} — start it with `xiaoguai serve`, \
             or pass --mock / --ollama-url for a direct backend"
        )
    })?;
    let session = client
        .create_session(&remote::CreateSessionRequest {
            user_id,
            model,
            title: None,
        })
        .await?;
    eprintln!("session: {}", session.id);
    client
        .send_message(&session.id, &prompt, |ev| {
            render_remote_event(&ev);
            Ok(())
        })
        .await?;
    Ok(())
}

/// Read an API key from stdin (consumes to EOF, trims). Backs `--api-key-stdin`
/// so the key never lands in argv or shell history. Intended for piping, e.g.
/// `printf %s "$KEY" | xiaoguai provider register --api-key-stdin ...`.
fn read_api_key_from_stdin() -> Result<String> {
    use std::io::Read as _;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| anyhow::anyhow!("failed to read API key from stdin: {e}"))?;
    let key = buf.trim().to_string();
    if key.is_empty() {
        return Err(anyhow::anyhow!(
            "--api-key-stdin was set but stdin was empty; pipe the key, e.g. \
             `printf %s \"$KEY\" | xiaoguai provider register --api-key-stdin ...`"
        ));
    }
    Ok(key)
}

/// Split a comma-separated CLI value into a clean list (drops empty segments).
fn split_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Read one line of input from stdin (visible).
fn prompt_line() -> Result<String> {
    let mut s = String::new();
    // `read_line` returns Ok(0) at EOF (closed pipe / Ctrl-D), NOT an error —
    // without this check a `loop`-on-invalid prompt would spin forever.
    if std::io::stdin().read_line(&mut s).context("read stdin")? == 0 {
        return Err(anyhow::anyhow!("unexpected end of input"));
    }
    Ok(s)
}

/// Restores terminal echo on drop — covers the normal return, the `?` error
/// path, AND a panic (Drop runs during unwind). Ctrl-C is handled separately in
/// [`prompt_hidden`] (a signal terminates without unwinding, so Drop alone
/// wouldn't run).
struct EchoGuard(bool);
impl Drop for EchoGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = std::process::Command::new("stty").arg("echo").status();
        }
    }
}

/// Read one line with terminal echo disabled (Unix, via `stty`). On a
/// non-terminal stdin (piped) or where `stty` is unavailable the input is read
/// visibly. Echo is restored on every exit path including Ctrl-C: the blocking
/// read runs on a worker thread raced against `ctrl_c`, and an `EchoGuard`
/// covers error/panic.
async fn prompt_hidden() -> Result<String> {
    use std::io::IsTerminal as _;
    // Only toggle echo for a real TTY; on a pipe we read the key as-is (and
    // `stty` would just error to stderr).
    let echo_off = std::io::stdin().is_terminal()
        && std::process::Command::new("stty")
            .arg("-echo")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    let _guard = EchoGuard(echo_off);

    let read = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).map(|n| (n, s))
    });
    tokio::select! {
        joined = read => {
            let (n, s) = joined.context("join stdin read")?.context("read stdin")?;
            if echo_off {
                eprintln!(); // the user's Enter wasn't echoed — emit the newline.
            }
            if n == 0 {
                return Err(anyhow::anyhow!("unexpected end of input"));
            }
            Ok(s)
        }
        _ = tokio::signal::ctrl_c() => {
            // `_guard` restores echo as it drops on the way out.
            Err(anyhow::anyhow!("cancelled"))
        }
    }
}

/// `xiaoguai init` — interactive setup wizard. Picks a provider from the local
/// registry, takes its API key (hidden), optionally makes it the default model,
/// and persists via the (already-tested) `provider::update`.
async fn handle_init(config: Option<&str>, plaintext: bool) -> Result<()> {
    use std::io::Write as _;
    let repo = build_provider_repo(config).await?;
    let providers = repo.list().await?;
    if providers.is_empty() {
        return Err(anyhow::anyhow!(
            "no providers found — run `xiaoguai serve` once so the migrations seed the defaults"
        ));
    }

    println!("xiaoguai setup — configure a model provider\n");
    print!("{}", init::format_provider_menu(&providers));

    let idx = loop {
        eprint!("\nPick a provider to configure [1-{}]: ", providers.len());
        std::io::stderr().flush().ok();
        if let Some(i) = init::parse_selection(&prompt_line()?, providers.len()) {
            break i;
        }
        eprintln!("  please enter a number between 1 and {}", providers.len());
    };
    let chosen = &providers[idx];

    eprint!(
        "\n{} API key ({} — leave blank to keep the current one): ",
        chosen.name,
        if plaintext { "shown" } else { "hidden" }
    );
    std::io::stderr().flush().ok();
    // `--plaintext` reads with echo on (the user sees the key as they type);
    // the default hides it like a password prompt. Either way we echo a masked
    // confirmation below so "did my paste land?" is always answerable.
    let key_raw = if plaintext {
        prompt_line()?
    } else {
        prompt_hidden().await?
    };
    let key = {
        let k = key_raw.trim();
        if k.is_empty() {
            None
        } else {
            Some(k.to_string())
        }
    };
    if let Some(k) = &key {
        eprintln!("  ✓ key captured: {}", init::mask_key(k));
    }

    // Region-relevant providers (M2): the seeded endpoint may be wrong for the
    // user's account — MiniMax international (api.minimax.io) vs the CN platform,
    // Azure's per-deployment URL, Bedrock's AWS region. A correct key against the
    // wrong host still 401s. `UpdateArgs.endpoint` already exists (#213).
    let endpoint = match chosen.kind.as_str() {
        // MiniMax keys are region-bound and NOT interchangeable. Offer an
        // explicit region picker rather than a free-form URL, so CN-console
        // users don't silently keep the international default and 401 — the #1
        // fresh-install failure we kept hitting in the field.
        "minimax" => {
            eprintln!(
                "\n{} region — your API key is tied to one (the two are NOT interchangeable):",
                chosen.name
            );
            eprintln!("{}", init::MINIMAX_REGION_MENU);
            loop {
                eprint!("Pick region [1-2] (blank to keep {}): ", chosen.endpoint);
                std::io::stderr().flush().ok();
                let line = prompt_line()?;
                if line.trim().is_empty() {
                    break None;
                }
                if let Some(ep) = init::minimax_region_endpoint(&line) {
                    break Some(ep.to_string());
                }
                eprintln!("  please enter 1 or 2");
            }
        }
        // Azure (per-deployment URL) / Bedrock (AWS region) still need a
        // free-form endpoint — there's no small fixed set to pick from.
        "azure_openai" | "bedrock" => {
            eprint!(
                "\n{} endpoint (blank to keep {}): ",
                chosen.name, chosen.endpoint
            );
            std::io::stderr().flush().ok();
            let e = prompt_line()?;
            let e = e.trim();
            if e.is_empty() {
                None
            } else {
                Some(e.to_string())
            }
        }
        _ => None,
    };

    eprint!(
        "\nMake {} the default model (so you can skip --model)? [Y/n]: ",
        chosen.name
    );
    std::io::stderr().flush().ok();
    let make_default = init::parse_yes_no(&prompt_line()?, true);

    // Warn if promoting a keyless cloud provider to default — it'll 401 on the
    // first request. Ollama needs no key, so don't nag there.
    let has_key = key.is_some() || chosen.api_key.is_some() || chosen.api_key_env.is_some();
    if make_default && !has_key && chosen.kind.as_str() != "ollama" {
        eprintln!(
            "  ! {} has no API key — as the default it will fail to authenticate. \
             Re-run init with a key, or: xiaoguai provider update --id {} --api-key-stdin",
            chosen.name,
            chosen.id.as_str()
        );
    }

    let repo_ref: &dyn LlmProviderRepository = &repo;

    // L1: before making this provider primary (fallback_order=0), demote any
    // OTHER provider currently at 0 to 1. The router sorts by
    // (fallback_order, created_at), so two providers at 0 would let the
    // earlier-created one win — not the one the user just chose. Demoting keeps
    // the chosen provider the unique lowest.
    if make_default {
        for p in &providers {
            if p.id.as_str() != chosen.id.as_str() && p.fallback_order == 0 {
                provider::update(
                    repo_ref,
                    provider::UpdateArgs {
                        id: p.id.as_str().to_string(),
                        fallback_order: Some(1),
                        ..Default::default()
                    },
                )
                .await?;
            }
        }
    }

    let updated = provider::update(
        repo_ref,
        provider::UpdateArgs {
            id: chosen.id.as_str().to_string(),
            // fallback_order=0 makes this provider primary, so the router uses
            // its first model as the deployment default (see #214).
            fallback_order: if make_default { Some(0) } else { None },
            api_key: key.clone(),
            endpoint: endpoint.clone(),
            ..Default::default()
        },
    )
    .await?;

    eprintln!();
    if key.is_some() {
        eprintln!("✓ stored API key for {}", updated.name);
    }
    if endpoint.is_some() {
        eprintln!("✓ endpoint set to {}", updated.endpoint);
    }
    if make_default {
        let model = updated.models.first().map_or("(its model)", String::as_str);
        eprintln!(
            "✓ {} is now the default provider (default model: {model})",
            updated.name
        );
    }
    // T8.1: end the wizard with concrete next steps. An already-running
    // server still needs the restart hint; a fresh install needs "start it".
    let port = load_settings(config).map_or(7600, |s| s.server.port);
    eprintln!("\n(If the server is already running, restart it to pick up the change.)\n");
    eprintln!("{}", init::format_next_steps(port));
    Ok(())
}

async fn handle_provider(config: Option<&str>, action: ProviderCmd) -> Result<()> {
    let repo = build_provider_repo(config).await?;
    let repo: &dyn LlmProviderRepository = &repo;
    match action {
        ProviderCmd::Register {
            name,
            kind,
            endpoint,
            models,
            default_for,
            fallback_order,
            api_key_env,
            api_key_stdin,
        } => {
            let api_key = if api_key_stdin {
                Some(read_api_key_from_stdin()?)
            } else {
                None
            };
            let p = provider::register(
                repo,
                provider::RegisterArgs {
                    name,
                    kind,
                    endpoint,
                    models,
                    default_for: default_for.into_iter().filter(|s| !s.is_empty()).collect(),
                    fallback_order,
                    api_key_env,
                    api_key,
                },
            )
            .await?;
            println!("registered {} ({})", p.id, p.name);
        }
        ProviderCmd::Update {
            id,
            endpoint,
            models,
            default_for,
            fallback_order,
            api_key_env,
            api_key_stdin,
        } => {
            let api_key = if api_key_stdin {
                Some(read_api_key_from_stdin()?)
            } else {
                None
            };
            let p = provider::update(
                repo,
                provider::UpdateArgs {
                    id,
                    endpoint,
                    models: models.as_deref().map(split_csv),
                    default_for: default_for.as_deref().map(split_csv),
                    fallback_order,
                    api_key_env,
                    api_key,
                },
            )
            .await?;
            println!("updated {} ({})", p.id, p.name);
        }
        ProviderCmd::List => {
            let rows = provider::list(repo, provider::ListArgs {}).await?;
            print!("{}", provider::format_table(&rows));
        }
        ProviderCmd::Remove { id } => {
            provider::remove(repo, provider::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

async fn handle_schedule(config: Option<&str>, action: ScheduleCmd) -> Result<()> {
    use std::sync::Arc;
    use xiaoguai_audit::chain::sink::SqliteAuditSink;
    use xiaoguai_scheduler::{SqliteJobRepository, SqliteJobRunRepository};

    let settings = load_settings(config)?;
    let pool = connect(&settings.database.url, settings.database.max_connections)
        .await
        .context("open SQLite store")?;
    // Idempotent migrate-on-connect (same pattern as `build_provider_repo`,
    // #221) so `schedule` works on a brand-new DB before the first `serve`.
    migrate(&pool).await.context("apply migrations")?;
    let jobs = SqliteJobRepository::new(pool.clone());
    let runs = SqliteJobRunRepository::new(pool.clone());
    // Sign schedule.* rows with the same key the server uses so CLI-written
    // rows verify in the same chain (same key resolution as `xiaoguai code`).
    // SEC-15: fail-closed — never fall back to the published dev key.
    let key = xiaoguai_cli::commands::resolve_audit_signing_key(&settings)?;
    let audit = schedule::SinkAuditAppender::new(Arc::new(SqliteAuditSink::new(pool, key)));

    match action {
        ScheduleCmd::Create {
            name,
            cron,
            prompt,
            description,
            sinks,
        } => {
            let job = schedule::create(
                &jobs,
                &audit,
                schedule::CreateArgs {
                    name,
                    cron,
                    prompt,
                    description,
                    sinks,
                },
            )
            .await?;
            println!(
                "created {} ({}) — next fire {}",
                job.id,
                job.name,
                job.next_fire_at
                    .map_or_else(|| "-".to_string(), |t| t.to_rfc3339())
            );
        }
        ScheduleCmd::List { limit } => {
            let rows = schedule::list(&jobs, &runs, limit).await?;
            if rows.is_empty() {
                println!("no scheduled jobs — create one with `xiaoguai schedule create`");
            } else {
                print!("{}", schedule::format_table(&rows));
            }
        }
        ScheduleCmd::Show { id } => {
            let (job, history) = schedule::show(&jobs, &runs, &id).await?;
            print!("{}", schedule::format_detail(&job, &history));
        }
        ScheduleCmd::Pause { id } => {
            let job = schedule::set_enabled(&jobs, &audit, &id, false).await?;
            println!("paused {} ({})", job.id, job.name);
        }
        ScheduleCmd::Resume { id } => {
            let job = schedule::set_enabled(&jobs, &audit, &id, true).await?;
            println!(
                "resumed {} ({}) — next fire {}",
                job.id,
                job.name,
                job.next_fire_at
                    .map_or_else(|| "-".to_string(), |t| t.to_rfc3339())
            );
        }
        ScheduleCmd::Delete { id, yes } => {
            let job = schedule::resolve(&jobs, &id).await?;
            if !yes {
                eprint!(
                    "Delete scheduled job '{}' ({}) and its run history? [y/N] ",
                    job.name, job.id
                );
                if !init::parse_yes_no(&prompt_line()?, false) {
                    println!("aborted — nothing deleted");
                    return Ok(());
                }
            }
            let gone = schedule::delete(&jobs, &audit, &job.id).await?;
            println!("deleted {} ({})", gone.id, gone.name);
        }
        ScheduleCmd::RunNow { id, server } => {
            // Resolve short ids against the local store; the REST route
            // needs the exact id.
            let job = schedule::resolve(&jobs, &id).await?;
            schedule::run_now(&server, &job.id).await?;
            println!(
                "fired {} ({}) — `xiaoguai schedule show {}` shows the run result",
                job.id,
                job.name,
                schedule::short_id(&job.id)
            );
        }
    }
    Ok(())
}

async fn handle_mcp(config: Option<&str>, action: McpCmd) -> Result<()> {
    let repo = build_mcp_repo(config).await?;
    let repo: &dyn McpServerRepository = &repo;
    match action {
        McpCmd::Register {
            name,
            version,
            transport,
            command,
            args,
            env_keys,
            endpoint,
            auth,
            auth_url,
            token_url,
            client_id,
            scopes,
        } => {
            let args: Vec<String> = args.into_iter().filter(|s| !s.is_empty()).collect();
            let env_keys: Vec<String> = env_keys.into_iter().filter(|s| !s.is_empty()).collect();
            let scopes: Vec<String> = scopes.into_iter().filter(|s| !s.is_empty()).collect();
            match auth.as_deref() {
                None | Some("none") => {
                    let server = mcp::register(
                        repo,
                        mcp::RegisterArgs {
                            name,
                            version,
                            transport,
                            command,
                            args,
                            env_keys,
                            endpoint,
                        },
                    )
                    .await?;
                    println!(
                        "registered {} ({}@{})",
                        server.id, server.name, server.version
                    );
                }
                Some("oauth2-pkce") => {
                    use std::sync::Arc;
                    use xiaoguai_mcp::auth::{InMemoryTokenStore, TokenStore};
                    let (listener, redirect_uri) = mcp::bind_callback_listener().await?;
                    let base = mcp::RegisterArgs {
                        name,
                        version,
                        transport,
                        command,
                        args,
                        env_keys,
                        endpoint,
                    };
                    let oauth_args = mcp::OAuthRegisterArgs {
                        auth_url: auth_url.unwrap_or_default(),
                        token_url: token_url.unwrap_or_default(),
                        client_id: client_id.unwrap_or_default(),
                        scopes,
                    };
                    // In-memory store for the consent flow; production
                    // wiring of a SqliteTokenStore is a follow-up (see
                    // docs/plans/2026-05-29-tier3-oauth-pkce-outbound-mcp.md §7).
                    let store: Arc<dyn TokenStore> = Arc::new(InMemoryTokenStore::new());
                    let (server, bundle, _oauth_cfg) = mcp::register_oauth_with_listener(
                        repo,
                        store,
                        listener,
                        redirect_uri,
                        base,
                        oauth_args,
                    )
                    .await?;
                    println!(
                        "registered {} ({}@{})",
                        server.id, server.name, server.version
                    );
                    println!("oauth: access_token expires {}", bundle.expires_at);
                }
                Some(other) => {
                    return Err(anyhow::anyhow!(
                        "unknown --auth value {other:?}: expected 'oauth2-pkce' or 'none'"
                    ));
                }
            }
        }
        McpCmd::List => {
            let rows = mcp::list(repo, mcp::ListArgs {}).await?;
            print!("{}", mcp::format_table(&rows));
        }
        McpCmd::Remove { id } => {
            mcp::remove(repo, mcp::RemoveArgs { id: id.clone() }).await?;
            println!("removed {id}");
        }
    }
    Ok(())
}

/// Render one streamed `RemoteEvent` to the terminal: assistant text to stdout
/// (so it pipes cleanly), tool/done/error markers to stderr. Shared by
/// `remote chat` and `repl`.
/// Render one `OrchestrateEvent` SSE frame: member/synthesis progress to
/// stderr, the synthesized answer to stdout (so it stays pipe-clean).
fn render_orchestrate_event(ev: &remote::RemoteEvent) {
    use std::io::Write as _;
    let p = &ev.payload;
    match ev.name.as_str() {
        "run_started" => {
            let n = p
                .get("members")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            eprintln!("\n▶ team run — {n} member(s) working in parallel");
        }
        "member_completed" => {
            let id = p
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let ok = p
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            eprintln!("  {} {id}", if ok { "✓" } else { "✗" });
        }
        "synthesis_started" => {
            let n = p
                .get("ok_members")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            eprintln!("▶ lead synthesizing from {n} member result(s)…");
        }
        "final" => {
            if let Some(text) = p.get("text").and_then(serde_json::Value::as_str) {
                println!("{text}");
                std::io::stdout().flush().ok();
            }
            let ok = p
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let failed = p
                .get("failed_members")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len);
            eprintln!("[done] ok={ok}, failed_members={failed}");
        }
        "error" => {
            let msg = p
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            eprintln!("[error] {msg}");
        }
        _ => {}
    }
}

fn render_remote_event(ev: &remote::RemoteEvent) {
    use std::io::Write as _;
    match ev.name.as_str() {
        "text_delta" => {
            if let Some(delta) = ev.payload.get("delta").and_then(serde_json::Value::as_str) {
                print!("{delta}");
                std::io::stdout().flush().ok();
            }
        }
        "tool_call_started" => {
            let name = ev
                .payload
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            eprintln!("{}", style::dim(&format!("\n[tool start] {name}")));
        }
        "tool_call_finished" => {
            let ok = ev
                .payload
                .get("ok")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let line = format!("[tool finish] ok={ok}");
            eprintln!(
                "{}",
                if ok {
                    style::dim(&line)
                } else {
                    style::warn(&line)
                }
            );
            // Show what the tool did — diffs get red/green backgrounds (removed
            // old → red, added new → green). Capped so a big result can't flood.
            if let Some(out) = ev
                .payload
                .get("output_text")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                const MAX_LINES: usize = 60;
                let total = out.lines().count();
                let shown = out.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
                eprintln!("{}", style::diff(&shown));
                if total > MAX_LINES {
                    eprintln!(
                        "{}",
                        style::dim(&format!("  … ({} more lines)", total - MAX_LINES))
                    );
                }
            }
        }
        "done" => {
            println!();
            let reason = ev
                .payload
                .get("stop_reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            eprintln!("{}", style::ok(&format!("[done] {reason}")));
        }
        "error" => {
            let msg = ev
                .payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            eprintln!("{}", style::err(&format!("[error] {msg}")));
        }
        _ => {}
    }
}

/// Interactive multi-turn REPL against a running server. Creates one session
/// and loops: read a line from stdin → stream the reply. `/exit`, `/quit`, or
/// EOF (Ctrl-D) quits. The prompt marker goes to stderr so the assistant's
/// stdout text stays pipe-clean.
/// The xiaoguai mascot + wordmark printed when the interactive CLI starts.
const CLI_LOGO: &str = r"
    \\   //
   ( o   o )    xiaoguai · 小怪
    \  -  /     Your Little Agent for Big Work
     \___/      小怪不小，能办大事
";

async fn handle_repl(server: String, user_id: String, model: String) -> Result<()> {
    use std::io::Write as _;
    let client = remote::RemoteClient::new(server.clone());
    client.healthz().await.with_context(|| {
        format!("could not reach the server at {server} — start it with `xiaoguai serve`")
    })?;
    // The session is created with `model`; `current_model` tracks the live
    // choice so `/model <name>` can switch it per-message without a reconnect.
    let mut current_model = model.clone();
    // Persistent CLI prefs (~/.xiaoguai/cli.json): the prompt marker + default
    // reply language, remembered across restarts. Mutable in-session via /config.
    let mut cfg = cli_config::load();
    // When a language is configured, prepend a one-time directive to the first
    // user turn so the agent replies in it (CLI-side; no server change needed).
    let mut lang_directive: Option<&'static str> = cli_config::language_directive(&cfg.language);
    let session = client
        .create_session(&remote::CreateSessionRequest {
            user_id,
            model,
            title: None,
        })
        .await?;
    eprintln!("{}", style::accent(CLI_LOGO));
    eprintln!(
        "{}",
        style::dim(&format!(
            "session {} — type /help for commands (/config to customise), /exit or Ctrl-D to quit",
            session.id
        ))
    );

    let stdin = std::io::stdin();
    loop {
        eprint!("\n{} ", style::prompt(&cfg.prompt));
        std::io::stderr().flush().ok();
        let mut line = String::new();
        if stdin.read_line(&mut line).context("read stdin")? == 0 {
            eprintln!();
            break; // EOF / Ctrl-D
        }
        match repl::parse_command(&line, &current_model) {
            repl::ReplAction::Quit => break,
            repl::ReplAction::Notice(msg) => eprintln!("{msg}"),
            repl::ReplAction::Clear => {
                // ANSI: clear screen + home the cursor.
                print!("\x1b[2J\x1b[H");
                std::io::stdout().flush().ok();
            }
            repl::ReplAction::SetModel(m) => {
                current_model = m;
                eprintln!("{}", style::ok(&format!("  ✓ model → {current_model}")));
            }
            repl::ReplAction::ConfigShow => eprintln!("{}", cli_config::render(&cfg)),
            repl::ReplAction::ConfigSet { key, value } => {
                match cli_config::apply_set(&cfg, &key, &value) {
                    Ok(next) => {
                        cfg = next;
                        // A language change re-arms the directive for the next turn.
                        if key == "language" || key == "lang" {
                            lang_directive = cli_config::language_directive(&cfg.language);
                        }
                        match cli_config::save(&cfg) {
                            Ok(()) => {
                                eprintln!(
                                    "{}",
                                    style::ok(&format!("  ✓ {key} → {}", value.trim()))
                                );
                            }
                            Err(e) => eprintln!(
                                "{}",
                                style::warn(&format!(
                                    "  ! applied in-session but could not persist: {e:#}"
                                ))
                            ),
                        }
                    }
                    Err(msg) => eprintln!("{}", style::warn(&format!("  ! {msg}"))),
                }
            }
            repl::ReplAction::Send(prompt) => {
                if prompt.is_empty() {
                    continue;
                }
                // One-time language directive prepended to the first turn so the
                // agent adopts the configured reply language.
                let content = match lang_directive.take() {
                    Some(d) => format!("{d}\n\n{prompt}"),
                    None => prompt,
                };
                let model_override = (!current_model.is_empty()).then_some(current_model.as_str());
                if let Err(e) = client
                    .send_message_with_model(&session.id, &content, model_override, |ev| {
                        render_remote_event(&ev);
                        Ok(())
                    })
                    .await
                {
                    // Keep the REPL alive on a per-turn error (network blip, etc.).
                    eprintln!("{}", style::err(&format!("[error] {e:#}")));
                }
            }
        }
    }
    eprintln!("bye");
    Ok(())
}

async fn handle_remote(server: String, action: RemoteCmd) -> Result<()> {
    let client = remote::RemoteClient::new(server);
    match action {
        RemoteCmd::Healthz => {
            let body = client.healthz().await?;
            println!("{body}");
        }
        RemoteCmd::Chat {
            user_id,
            model,
            prompt,
            title,
        } => {
            let session = client
                .create_session(&remote::CreateSessionRequest {
                    user_id,
                    model,
                    title,
                })
                .await?;
            eprintln!("session: {}", session.id);
            client
                .send_message(&session.id, &prompt, |ev| {
                    render_remote_event(&ev);
                    Ok(())
                })
                .await?;
        }
        RemoteCmd::Messages { session } => {
            let msgs = client.list_messages(&session).await?;
            println!("{}", serde_json::to_string_pretty(&msgs)?);
        }
        RemoteCmd::Cancel { session } => {
            let cancelled = client.cancel(&session).await?;
            println!("cancelled={cancelled}");
        }
        RemoteCmd::Orchestrate {
            user_id,
            goal,
            team,
            max_members,
        } => {
            let session = client
                .create_session(&remote::CreateSessionRequest {
                    user_id,
                    model: String::new(),
                    title: None,
                })
                .await?;
            eprintln!("session: {}", session.id);
            client
                .orchestrate(
                    &session.id,
                    &remote::OrchestrateRequest {
                        goal,
                        team_id: team,
                        max_members,
                    },
                    |ev| {
                        render_orchestrate_event(&ev);
                        Ok(())
                    },
                )
                .await?;
        }
    }
    Ok(())
}

async fn handle_loop(server: String, action: LoopCmd) -> Result<()> {
    let client = remote::RemoteClient::new(server);
    match action {
        LoopCmd::Create {
            session,
            prompt,
            interval_secs,
            max_ticks,
            ttl_secs,
            dynamic_pacing,
            min_interval_secs,
            max_interval_secs,
            max_total_tokens,
        } => {
            let row = client
                .create_loop(&remote::CreateLoopRequest {
                    session_id: session,
                    prompt,
                    interval_secs,
                    max_ticks,
                    ttl_secs,
                    dynamic_pacing,
                    min_interval_secs,
                    max_interval_secs,
                    max_total_tokens,
                })
                .await?;
            println!("{}", r#loop::format_detail(&row));
        }
        LoopCmd::List => {
            let rows = client.list_loops().await?;
            print!("{}", r#loop::format_table(&rows));
        }
        LoopCmd::Show { id } => {
            let id = resolve_loop_id(&client, &id).await?;
            let row = client.get_loop(&id).await?;
            println!("{}", r#loop::format_detail(&row));
        }
        LoopCmd::Cancel { id } => {
            let id = resolve_loop_id(&client, &id).await?;
            let row = client.cancel_loop(&id).await?;
            println!("loop {} cancelled (status: {})", row.id, row.status);
        }
        LoopCmd::Resume { id } => {
            let id = resolve_loop_id(&client, &id).await?;
            let row = client.resume_loop(&id).await?;
            println!("loop {} resumed (status: {})", row.id, row.status);
        }
    }
    Ok(())
}

/// Resolve a loop id or unique id prefix against the server's loop list —
/// the `list` table shows short ids, so accept any unambiguous prefix.
async fn resolve_loop_id(client: &remote::RemoteClient, id_or_prefix: &str) -> Result<String> {
    let needle = id_or_prefix.trim();
    if needle.is_empty() {
        anyhow::bail!("loop id must not be empty — run `xiaoguai loop list` to see ids");
    }
    let rows = client.list_loops().await?;
    let matches: Vec<&remote::LoopResponse> =
        rows.iter().filter(|r| r.id.starts_with(needle)).collect();
    match matches.as_slice() {
        [] => anyhow::bail!("no loop matches '{needle}' — run `xiaoguai loop list` to see ids"),
        [one] => Ok(one.id.clone()),
        many => {
            let ids: Vec<&str> = many.iter().map(|r| r.id.as_str()).collect();
            anyhow::bail!(
                "'{needle}' is ambiguous — matches {} loops:\n  {}\nUse a longer prefix.",
                many.len(),
                ids.join("\n  ")
            )
        }
    }
}

async fn handle_eval(action: EvalCmd) -> Result<()> {
    match action {
        EvalCmd::Run {
            suite,
            cases_dir,
            out,
            max_iterations,
        } => {
            let report = eval::run(eval::EvalArgs {
                suite,
                cases_dir: cases_dir.map(std::path::PathBuf::from),
                out: out.map(std::path::PathBuf::from),
                max_iterations,
            })
            .await?;
            print!("{}", xiaoguai_eval::pretty_summary(&report));
            if report.failed() > 0 {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers — wave-3
// ---------------------------------------------------------------------------

async fn handle_hotl(api_base: String, output: String, action: HotlCmd) -> Result<()> {
    match action {
        HotlCmd::Policy { action } => match action {
            HotlPolicyCmd::Create {
                scope,
                window_secs,
                max_count,
                max_usd,
                escalate_to,
            } => {
                let v = hotl::policy_create(hotl::PolicyCreateArgs {
                    api_base,
                    scope,
                    window_secs,
                    max_count,
                    max_usd,
                    escalate_to,
                })
                .await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::List { scope } => {
                let rows = hotl::policy_list(hotl::PolicyListArgs { api_base, scope }).await?;
                if output == "table" {
                    print!("{}", hotl::format_policy_table(&rows));
                } else {
                    print_value(&serde_json::to_value(&rows)?, &output)?;
                }
            }
            HotlPolicyCmd::Get { id } => {
                let v = hotl::policy_get(hotl::PolicyGetArgs { api_base, id }).await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::Update {
                id,
                max_count,
                max_usd,
                escalate_to,
                window_secs,
            } => {
                let v = hotl::policy_update(hotl::PolicyUpdateArgs {
                    api_base,
                    id,
                    max_count,
                    max_usd,
                    escalate_to,
                    window_secs,
                })
                .await?;
                print_value(&v, &output)?;
            }
            HotlPolicyCmd::Delete { id } => {
                let id_clone = id.clone();
                hotl::policy_delete(hotl::PolicyDeleteArgs { api_base, id }).await?;
                println!("deleted {id_clone}");
            }
        },
        HotlCmd::Check { scope, amount } => {
            let resp = hotl::check(hotl::CheckArgs {
                api_base,
                scope,
                amount,
            })
            .await?;
            println!("verdict: {}", resp.verdict);
            if let Some(reason) = resp.reason {
                println!("reason: {reason:?}");
            }
        }
        HotlCmd::Pending => {
            let rows = hotl::pending_list(&api_base).await?;
            if output == "table" {
                print!("{}", hotl::format_pending_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
    }
    Ok(())
}

async fn handle_outcomes(api_base: String, output: String, action: OutcomesCmd) -> Result<()> {
    match action {
        OutcomesCmd::Record {
            agent_name,
            kind,
            value,
            session_id,
            unit,
            description,
        } => {
            let v = outcomes::record(outcomes::RecordArgs {
                api_base,
                agent_name,
                kind,
                value,
                session_id,
                unit,
                description,
            })
            .await?;
            print_value(&v, &output)?;
        }
        OutcomesCmd::List { range, kind, limit } => {
            let rows = outcomes::list(outcomes::ListArgs {
                api_base,
                range,
                kind,
                limit,
            })
            .await?;
            if output == "table" {
                print!("{}", outcomes::format_list_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        OutcomesCmd::Summary { range } => {
            let rows = outcomes::summary(outcomes::SummaryArgs { api_base, range }).await?;
            if output == "table" {
                print!("{}", outcomes::format_summary_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        OutcomesCmd::Timeseries { range, kind } => {
            let v = outcomes::timeseries(outcomes::TimeseriesArgs {
                api_base,
                range,
                kind,
            })
            .await?;
            print_value(&v, &output)?;
        }
    }
    Ok(())
}

async fn handle_skills(api_base: String, output: String, action: SkillsCmd) -> Result<()> {
    match action {
        SkillsCmd::List {
            category,
            installed,
        } => {
            let rows = skills::list(skills::ListArgs {
                api_base,
                category,
                installed,
            })
            .await?;
            if output == "table" {
                if installed {
                    print!("{}", skills::format_installed_table(&rows));
                } else {
                    print!("{}", skills::format_catalog_table(&rows));
                }
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        SkillsCmd::Install { pack, config } => {
            let v = skills::install(skills::InstallArgs {
                api_base,
                pack,
                config,
            })
            .await?;
            print_value(&v, &output)?;
        }
        SkillsCmd::InstallFromFile { .. } => {
            skills::install_from_file_not_implemented()?;
        }
        SkillsCmd::Uninstall { id } => {
            skills::uninstall(skills::UninstallArgs {
                api_base,
                id: id.clone(),
            })
            .await?;
            println!("{}", serde_json::json!({"ok": true}));
        }
        SkillsCmd::Proposals { action } => {
            handle_proposals(api_base, output, action).await?;
        }
    }
    Ok(())
}

async fn handle_proposals(api_base: String, output: String, action: ProposalsCmd) -> Result<()> {
    match action {
        ProposalsCmd::List { status } => {
            let rows = skills::proposals_list(&api_base, status.as_deref()).await?;
            if output == "table" {
                print!("{}", skills::format_proposals_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        ProposalsCmd::Approve { id, decided_by } => {
            let v = skills::proposals_approve(&api_base, &id, &decided_by).await?;
            print_value(&v, &output)?;
        }
        ProposalsCmd::Reject {
            id,
            decided_by,
            reason,
        } => {
            let v = skills::proposals_reject(&api_base, &id, &decided_by, &reason).await?;
            print_value(&v, &output)?;
        }
    }
    Ok(())
}

async fn handle_watch(api_base: String, output: String, action: WatchCmd) -> Result<()> {
    match action {
        WatchCmd::List => {
            let rows = watch::list(watch::ListArgs { api_base }).await?;
            if output == "table" {
                print!("{}", watch::format_list_table(&rows));
            } else {
                print_value(&serde_json::to_value(&rows)?, &output)?;
            }
        }
        WatchCmd::Start { file } => {
            let v = watch::start(watch::StartArgs {
                api_base,
                file: std::path::PathBuf::from(file),
            })
            .await?;
            print_value(&v, &output)?;
        }
        WatchCmd::Stop { id } => {
            let id_clone = id.clone();
            watch::stop(watch::StopArgs { api_base, id }).await?;
            println!("stopped: {id_clone}");
        }
        WatchCmd::Test { id } => {
            let v = watch::test(watch::TestArgs { api_base, id }).await?;
            print_value(&v, &output)?;
        }
    }
    Ok(())
}

async fn handle_anomaly(api_base: String, output: String, action: AnomalyCmd) -> Result<()> {
    match action {
        AnomalyCmd::Run { file } => {
            let v = anomaly::run(anomaly::RunArgs {
                api_base,
                file: std::path::PathBuf::from(file),
            })
            .await?;
            print_value(&v, &output)?;
        }
        AnomalyCmd::Test {
            file,
            data,
            ts_col,
            val_col,
        } => {
            let result = anomaly::backtest(anomaly::BacktestArgs {
                api_base,
                file: std::path::PathBuf::from(file),
                data: std::path::PathBuf::from(data),
                ts_col,
                val_col,
            })
            .await?;
            if output == "table" {
                print!("{}", anomaly::format_backtest_table(&result));
            } else {
                print_value(&result, &output)?;
            }
        }
    }
    Ok(())
}

async fn handle_pack(config: Option<&str>, action: PackCmd) -> Result<()> {
    match action {
        PackCmd::Validate { dir } => {
            let path = std::path::Path::new(&dir);
            // A single pack (a pack.yaml, or a dir holding one) prints its report;
            // a load/validation failure propagates via `?` → non-zero exit.
            if pack::is_single_pack(path) {
                print!("{}", pack::validate(path).await?);
            } else {
                // Otherwise treat `dir` as a parent of many packs: validate each
                // and exit non-zero if any failed (a CI gate over the corpus).
                let outcome = pack::validate_all(path).await?;
                print!("{}", outcome.report);
                anyhow::ensure!(
                    outcome.failed == 0,
                    "{} of {} pack(s) failed validation",
                    outcome.failed,
                    outcome.total
                );
            }
        }
        PackCmd::Install { dir } => {
            let path = std::path::Path::new(&dir);
            let settings = load_settings(config)?;
            let pool = connect(&settings.database.url, settings.database.max_connections)
                .await
                .context("open SQLite store")?;
            migrate(&pool).await.context("apply migrations")?;
            print!("{}", pack::install(&pool, path).await?);
        }
    }
    Ok(())
}

async fn handle_tasks(api_base: String, action: TasksCmd) -> Result<()> {
    let client = tasks::TasksClient::new(api_base);
    match action {
        TasksCmd::List { board, column } => {
            let result = client.list(&board, column.as_deref()).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Create {
            title,
            description,
            board,
            column,
        } => {
            let req = tasks::CreateTaskRequest {
                title,
                description,
                board,
                column,
            };
            let result = client.create(&req).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Move { task_id, to } => {
            let result = client.move_task(&task_id, &to).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Claim { task_id, agent } => {
            let result = client.claim(&task_id, agent.as_deref()).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Complete { task_id, outcome } => {
            let result = client.complete(&task_id, outcome.as_deref()).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Block { task_id, reason } => {
            let result = client.block(&task_id, &reason).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Dispatch { board, n } => {
            let result = client.dispatch(&board, n).await;
            match result {
                Ok(v) => {
                    // Server may return empty array when no READY cards exist.
                    if v.as_array().is_some_and(Vec::is_empty) {
                        println!("no ready cards on board '{board}'");
                    } else {
                        println!("{}", tasks::pretty(&v));
                    }
                }
                Err(e) => println!("{e}"),
            }
        }
        TasksCmd::Show { task_id } => {
            let result = client.show(&task_id).await;
            match result {
                Ok(v) => println!("{}", tasks::pretty(&v)),
                Err(e) => println!("{e}"),
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Output helper
// ---------------------------------------------------------------------------

fn print_value(v: &serde_json::Value, format: &str) -> Result<()> {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(v)?),
        "yaml" => print!("{}", serde_yaml::to_string(v)?),
        _ => println!("{}", serde_json::to_string_pretty(v)?),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // ACP owns stdout for its JSON-RPC stream, so its logs MUST go to stderr;
    // every other subcommand keeps the default stdout logger.
    if matches!(cli.command, Cmd::Acp) {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt::init();
    }
    let cfg = cli.config.as_deref();
    match cli.command {
        Cmd::Serve { host, port } => {
            let mut settings = xiaoguai_core::load_settings(cfg.map(std::path::Path::new))
                .context("load settings for serve")?;
            // Flag > env > config: CLI flags override the already-merged settings
            // for a one-step `xiaoguai serve --host 0.0.0.0` LAN launch.
            if let Some(h) = host {
                settings.server.host = h;
            }
            if let Some(p) = port {
                settings.server.port = p;
            }
            xiaoguai_core::run_serve(&settings).await
        }
        Cmd::Smoke => {
            let settings = xiaoguai_core::load_settings(cfg.map(std::path::Path::new))
                .context("load settings for smoke")?;
            xiaoguai_core::run_smoke(&settings).await
        }
        Cmd::Acp => {
            let settings = xiaoguai_core::load_settings(cfg.map(std::path::Path::new))
                .context("load settings for acp")?;
            xiaoguai_core::acp_bridge::run_acp(&settings).await
        }
        Cmd::Chat {
            prompt,
            server,
            user_id,
            mock,
            ollama_url,
            model,
        } => handle_chat(prompt, server, user_id, mock, ollama_url, model).await,
        Cmd::Provider { action } => handle_provider(cfg, action).await,
        Cmd::Init { plaintext } => handle_init(cfg, plaintext).await,
        Cmd::Doctor => {
            let settings = load_settings(cfg)?;
            let results = doctor::run(&settings).await;
            print!("{}", doctor::format_report(&results));
            if doctor::has_failure(&results) {
                // Hard ✗ only — warns (e.g. model not pulled) stay exit 0.
                std::process::exit(1);
            }
            Ok(())
        }
        Cmd::Service { action } => {
            let action = match action {
                ServiceCmd::Install { print_only } => service::Action::Install { print_only },
                ServiceCmd::Uninstall => service::Action::Uninstall,
                ServiceCmd::Status => service::Action::Status,
            };
            service::run(action)
        }
        Cmd::Mcp { action } => handle_mcp(cfg, action).await,
        Cmd::Schedule { action } => handle_schedule(cfg, action).await,
        Cmd::Remote { server, action } => handle_remote(server, action).await,
        Cmd::Loop { server, action } => handle_loop(server, action).await,
        Cmd::Repl {
            server,
            user_id,
            model,
        } => handle_repl(server, user_id, model).await,
        Cmd::Eval { action } => handle_eval(action).await,
        Cmd::Completions { shell } => {
            let mut cmd = Cli::command();
            completions::run(shell, &mut cmd, &mut std::io::stdout())
        }
        Cmd::Manpages { outdir } => {
            let mut cmd = Cli::command();
            let written = manpages::run(&mut cmd, std::path::Path::new(&outdir))?;
            for p in written {
                println!("wrote {}", p.display());
            }
            Ok(())
        }
        Cmd::Backup {
            out,
            database_url,
            encrypt,
        } => {
            let out_path = backup::run_backup(backup::BackupArgs {
                out: std::path::PathBuf::from(out),
                database_url,
                encrypt: encrypt.map(std::path::PathBuf::from),
            })?;
            println!("backup written to {}", out_path.display());
            Ok(())
        }
        Cmd::Restore {
            input,
            outdir,
            force,
            identity,
            restore_db,
        } => {
            let restore_db_to = restore_db.map(|url| backup::resolve_sqlite_path(&url));
            backup::run_restore(backup::RestoreArgs {
                input: std::path::PathBuf::from(input),
                outdir: std::path::PathBuf::from(outdir),
                force,
                identity: identity.map(std::path::PathBuf::from),
                restore_db_to,
            })?;
            println!("restore complete");
            Ok(())
        }
        Cmd::Memory { action } => handle_memory(cli.config.as_deref(), action).await,
        Cmd::SelfUpdate { check } => {
            self_update::run_self_update(self_update::SelfUpdateArgs {
                check,
                api_url: None,
            })
            .await
        }
        Cmd::Stats {
            by,
            since,
            until,
            json,
        } => handle_stats(cfg, by, since, until, json).await,
        Cmd::Code { workspace, action } => {
            let settings = load_settings(cfg)?;
            let ws = std::path::Path::new(&workspace);
            match action {
                CodeCmd::Status => code::status(&settings, ws).await,
                CodeCmd::Write { path, content } => {
                    code::write(&settings, ws, std::path::Path::new(&path), content).await
                }
                CodeCmd::Commit { message } => code::commit(&settings, ws, message).await,
                CodeCmd::Rollback { checkpoint } => code::rollback(&settings, ws, checkpoint).await,
                CodeCmd::Push { branch, remote } => code::push(&settings, ws, remote, branch).await,
                CodeCmd::OpenPr { title, body, base } => {
                    code::open_pr(&settings, ws, title, body, base).await
                }
            }
        }
        // Wave-3
        Cmd::Hotl {
            api_base,
            output,
            action,
        } => handle_hotl(api_base, output, action).await,
        Cmd::Outcomes {
            api_base,
            output,
            action,
        } => handle_outcomes(api_base, output, action).await,
        Cmd::Skills {
            api_base,
            output,
            action,
        } => handle_skills(api_base, output, action).await,
        Cmd::Watch {
            api_base,
            output,
            action,
        } => handle_watch(api_base, output, action).await,
        Cmd::Anomaly {
            api_base,
            output,
            action,
        } => handle_anomaly(api_base, output, action).await,
        // Skill-pack manifest tooling (offline; Phase 1 = validate).
        Cmd::Pack { action } => handle_pack(cfg, action).await,
        // v1.4 — Kanban task board
        Cmd::Tasks { api_base, action } => handle_tasks(api_base, action).await,
        // T5 (Tier-3) — compliance export.
        Cmd::Audit { api_base, action } => handle_audit(api_base, action).await,
    }
}

async fn handle_audit(api_base: String, action: AuditCmd) -> Result<()> {
    match action {
        AuditCmd::Export {
            framework,
            from,
            to,
            output,
            format,
        } => {
            audit_export::run(audit_export::ExportArgs {
                api_base,
                framework,
                from,
                to,
                output: std::path::PathBuf::from(output),
                format,
            })
            .await
        }
        AuditCmd::Bundle {
            framework,
            from,
            to,
            out,
        } => {
            audit_bundle::run(audit_bundle::BundleArgs {
                api_base,
                framework,
                from,
                to,
                out_dir: std::path::PathBuf::from(out),
            })
            .await
        }
    }
}
