//! cokacconvert CLI — thin wrapper over the cokacmux library.

use std::fs::{self, OpenOptions};
use std::io::{IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};

use cokacmux::{
    providers, read_session, session, universal::Provider as LibProvider, write_session,
    SessionSource, SessionTarget, UniversalSession,
};

const APP_DIR_NAME: &str = ".cokacmux";
const DEBUG_LOG_FILE: &str = "cokacmux.log";
const DEBUG_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Parser, Debug)]
#[command(
    name = "cokacconvert",
    version,
    about = "Convert and manage coding-agent session data (Claude Code / Codex / OpenCode)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Parse a session and emit the UniversalType JSON to stdout.
    Inspect {
        #[arg(long, value_enum, default_value_t = ProviderArg::Auto)]
        from: ProviderArg,
        #[arg(long)]
        input: String,
        #[arg(long)]
        cwd: Option<String>,
    },

    /// Convert a session from one provider's format to another.
    Convert {
        #[arg(long, value_enum)]
        from: ProviderArg,
        #[arg(long, value_enum)]
        to: ProviderArg,
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
    },

    /// List sessions stored by a single provider.
    List {
        #[arg(long, value_enum)]
        provider: ProviderArg,
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Cross-provider session manager.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCmd {
    /// List sessions across all providers (or filtered).
    Ls {
        /// Limit to a single provider.
        #[arg(long, value_enum)]
        provider: Option<ProviderArg>,
        /// Only show sessions whose cwd equals this.
        #[arg(long)]
        cwd: Option<String>,
        /// Only show sessions whose title contains this substring.
        #[arg(long)]
        title_contains: Option<String>,
        /// Show clone parent/child hierarchy instead of a flat recent list.
        #[arg(long)]
        tree: bool,
        /// Max rows.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Show a session as a human-readable transcript.
    Show {
        /// Session id (or unique prefix).
        id: String,
        /// Full text (no truncation). Default is a summary view.
        #[arg(long)]
        full: bool,
        /// Emit the raw UniversalSession JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
    /// Clone a session with a fresh id.
    Clone {
        /// Source session id (or unique prefix).
        id: String,
        /// Target provider (default: same as source).
        #[arg(long, value_enum)]
        to: Option<ProviderArg>,
        /// Override cwd on the new session.
        #[arg(long)]
        cwd: Option<String>,
        /// Use this specific new id (otherwise UUID v7).
        #[arg(long)]
        new_id: Option<String>,
        /// Overwrite if the target already has a session with the new id.
        #[arg(long)]
        force: bool,
    },
    /// Delete a session from the agent's live storage.
    Rm {
        /// Session id (or unique prefix).
        id: String,
        /// Skip the confirmation prompt.
        #[arg(long, short)]
        yes: bool,
    },
    /// Search across all sessions for text content.
    Search {
        /// Search query (substring match).
        query: String,
        /// Case-sensitive (default: insensitive).
        #[arg(long)]
        case_sensitive: bool,
        /// Max rows.
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// Validate that a live session artifact matches its provider's native storage invariants.
    Validate {
        /// Session id (or unique prefix).
        id: String,
        /// Emit JSON report.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum ProviderArg {
    Claude,
    Codex,
    Opencode,
    Universal,
    Auto,
}

impl ProviderArg {
    fn to_lib(self) -> Result<LibProvider> {
        match self {
            ProviderArg::Claude => Ok(LibProvider::Claude),
            ProviderArg::Codex => Ok(LibProvider::Codex),
            ProviderArg::Opencode => Ok(LibProvider::OpenCode),
            other => Err(anyhow!(
                "{:?} is not a concrete provider (use claude/codex/opencode)",
                other
            )),
        }
    }
}

fn init_debug_from_env_or_settings() {
    let mux_env_enabled = std::env::var("COKACMUX_DEBUG")
        .map(|value| value == "1")
        .unwrap_or(false);
    let convert_env_enabled = std::env::var("COKACCONVERT_DEBUG")
        .map(|value| value == "1")
        .unwrap_or(false);
    let settings_enabled = settings_debug_enabled();
    let enabled = mux_env_enabled || convert_env_enabled || settings_enabled;
    DEBUG_ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        let source = if convert_env_enabled {
            "COKACCONVERT_DEBUG"
        } else if mux_env_enabled {
            "COKACMUX_DEBUG"
        } else {
            "settings"
        };
        debug_log_to(
            DEBUG_LOG_FILE,
            &format!(
                "debug enabled source={} settings_debug={}",
                source, settings_enabled
            ),
        );
    }
}

fn settings_debug_enabled() -> bool {
    let Some(path) = settings_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|value| {
            value
                .pointer("/cokacmux/debug")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

fn settings_path() -> Option<PathBuf> {
    app_config_dir().map(|dir| dir.join("settings.json"))
}

fn app_config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(APP_DIR_NAME))
}

fn debug_log(event: &str, details: serde_json::Value) {
    if !DEBUG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let msg = if details.as_object().is_some_and(|object| object.is_empty()) {
        event.to_string()
    } else {
        match serde_json::to_string(&details) {
            Ok(details) => format!("{} {}", event, details),
            Err(_) => event.to_string(),
        }
    };
    debug_log_to(debug_log_file_for(event), &msg);
}

fn debug_log_file_for(_event: &str) -> &'static str {
    DEBUG_LOG_FILE
}

fn debug_log_to(filename: &str, msg: &str) {
    if !DEBUG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(dir) = app_config_dir().map(|dir| dir.join("debug")) else {
        return;
    };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    #[cfg(unix)]
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));

    let path = dir.join(filename);
    if path
        .metadata()
        .map(|meta| meta.len() > DEBUG_LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        let rotated = dir.join(format!("{}.1", filename));
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, rotated);
    }

    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    #[cfg(unix)]
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    let thread_id = format!("{:?}", thread.id());
    let _ = writeln!(
        file,
        "[{} pid={} thread={} {}] {}",
        timestamp,
        std::process::id(),
        thread_name,
        thread_id,
        msg
    );
}

fn cmd_label(cmd: &Cmd) -> &'static str {
    match cmd {
        Cmd::Inspect { .. } => "inspect",
        Cmd::Convert { .. } => "convert",
        Cmd::List { .. } => "list",
        Cmd::Session { .. } => "session",
    }
}

fn session_cmd_label(cmd: &SessionCmd) -> &'static str {
    match cmd {
        SessionCmd::Ls { .. } => "ls",
        SessionCmd::Show { .. } => "show",
        SessionCmd::Clone { .. } => "clone",
        SessionCmd::Rm { .. } => "rm",
        SessionCmd::Search { .. } => "search",
        SessionCmd::Validate { .. } => "validate",
    }
}

fn provider_arg_label(provider: ProviderArg) -> &'static str {
    match provider {
        ProviderArg::Claude => "claude",
        ProviderArg::Codex => "codex",
        ProviderArg::Opencode => "opencode",
        ProviderArg::Universal => "universal",
        ProviderArg::Auto => "auto",
    }
}

fn parse_source(arg: &str, prov: LibProvider) -> Result<SessionSource> {
    if let Some((db, sid)) = arg.split_once('#') {
        if prov == LibProvider::OpenCode {
            debug_log(
                "parse_source",
                serde_json::json!({
                    "provider": prov.as_str(),
                    "kind": "opencode_db",
                    "db_path": db,
                    "session_id": sid,
                }),
            );
            return Ok(SessionSource::OpenCodeDb {
                db_path: PathBuf::from(db),
                session_id: sid.to_string(),
            });
        }
    }
    debug_log(
        "parse_source",
        serde_json::json!({
            "provider": prov.as_str(),
            "kind": "path",
            "path": arg,
        }),
    );
    Ok(SessionSource::Path(PathBuf::from(arg)))
}

fn detect_provider(input: &str) -> Result<LibProvider> {
    debug_log(
        "detect_provider_start",
        serde_json::json!({
            "input": input,
        }),
    );
    if let Some((db, _)) = input.split_once('#') {
        if std::path::Path::new(db)
            .extension()
            .and_then(|e| e.to_str())
            == Some("db")
        {
            debug_log(
                "detect_provider_ok",
                serde_json::json!({
                    "input": input,
                    "provider": LibProvider::OpenCode.as_str(),
                    "reason": "db_fragment",
                }),
            );
            return Ok(LibProvider::OpenCode);
        }
    }
    let p = PathBuf::from(input);
    if p.extension().and_then(|e| e.to_str()) == Some("db") {
        debug_log(
            "detect_provider_ok",
            serde_json::json!({
                "input": input,
                "provider": LibProvider::OpenCode.as_str(),
                "reason": "db_path",
            }),
        );
        return Ok(LibProvider::OpenCode);
    }
    let content = match fs::read_to_string(&p).with_context(|| format!("read {}", p.display())) {
        Ok(content) => content,
        Err(error) => {
            debug_log(
                "detect_provider_error",
                serde_json::json!({
                    "input": input,
                    "error": error.to_string(),
                }),
            );
            return Err(error);
        }
    };
    for line in content.lines().take(4) {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("payload").is_some() && v.get("type").is_some() {
            debug_log(
                "detect_provider_ok",
                serde_json::json!({
                    "input": input,
                    "provider": LibProvider::Codex.as_str(),
                    "reason": "codex_jsonl_shape",
                }),
            );
            return Ok(LibProvider::Codex);
        }
        if v.get("sessionId").is_some() || v.get("uuid").is_some() {
            debug_log(
                "detect_provider_ok",
                serde_json::json!({
                    "input": input,
                    "provider": LibProvider::Claude.as_str(),
                    "reason": "claude_jsonl_shape",
                }),
            );
            return Ok(LibProvider::Claude);
        }
    }
    debug_log(
        "detect_provider_error",
        serde_json::json!({
            "input": input,
            "error": "could not detect provider",
        }),
    );
    bail!("could not detect provider from {}", input)
}

fn write_universal_to(session: &UniversalSession, output: &str) -> Result<()> {
    let s = serde_json::to_string_pretty(session)?;
    debug_log(
        "convert_write_universal_start",
        serde_json::json!({
            "output": output,
            "messages": session.messages.len(),
            "bytes": s.len(),
        }),
    );
    if output == "-" {
        println!("{}", s);
    } else {
        fs::write(output, s)?;
    }
    debug_log(
        "convert_write_universal_ok",
        serde_json::json!({
            "output": output,
        }),
    );
    Ok(())
}

fn fmt_age(epoch_s: u64) -> String {
    if epoch_s == 0 {
        return "?".into();
    }
    let now = chrono::Utc::now().timestamp() as u64;
    let secs = now.saturating_sub(epoch_s);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 86_400 * 30 {
        format!("{}d", secs / 86_400)
    } else {
        let d: DateTime<Utc> = DateTime::from_timestamp(epoch_s as i64, 0).unwrap_or_default();
        d.format("%Y-%m-%d").to_string()
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut taken: String = s.chars().take(n.saturating_sub(1)).collect();
        taken.push('…');
        taken
    }
}

fn run_session(cmd: SessionCmd) -> Result<()> {
    debug_log(
        "session_cmd_start",
        serde_json::json!({
            "cmd": session_cmd_label(&cmd),
        }),
    );
    match cmd {
        SessionCmd::Ls {
            provider,
            cwd,
            title_contains,
            tree,
            limit,
        } => {
            debug_log(
                "session_ls_start",
                serde_json::json!({
                    "provider": provider.map(provider_arg_label),
                    "cwd": cwd.as_deref(),
                    "title_filter_len": title_contains.as_ref().map(|s| s.chars().count()),
                    "tree": tree,
                    "limit": limit,
                }),
            );
            let all = session::list_all()?;
            debug_log(
                "session_ls_loaded",
                serde_json::json!({
                    "total": all.len(),
                }),
            );
            let matches = |s: &cokacmux::providers::discovery::SessionInfo| {
                if let Some(p) = provider {
                    if let Ok(lp) = p.to_lib() {
                        if s.provider != lp {
                            return false;
                        }
                    }
                }
                if let Some(c) = &cwd {
                    if &s.cwd != c {
                        return false;
                    }
                }
                if let Some(t) = &title_contains {
                    let tl = t.to_lowercase();
                    let st = s.title.as_deref().unwrap_or("").to_lowercase();
                    if !st.contains(&tl) {
                        return false;
                    }
                }
                true
            };
            println!(
                "{:<8} {:<38} {:<6} {:<24} {:<30}",
                "PROV", "SESSION_ID", "AGE", "TITLE", "CWD"
            );
            let mut printed = 0usize;
            if tree {
                let links = match session::clone_tree::load_links() {
                    Ok(links) => {
                        debug_log(
                            "session_ls_tree_links_ok",
                            serde_json::json!({
                                "links": links.len(),
                            }),
                        );
                        links
                    }
                    Err(error) => {
                        debug_log(
                            "session_ls_tree_links_error",
                            serde_json::json!({
                                "error": error.to_string(),
                            }),
                        );
                        Default::default()
                    }
                };
                for row in session::clone_tree::visible_tree_rows(&all, &links, matches)
                    .into_iter()
                    .take(limit)
                {
                    printed = printed.saturating_add(1);
                    let indent = tree_indent(row.depth);
                    let session_id = format!("{}{}", indent, row.info.session_id);
                    println!(
                        "{:<8} {:<38} {:<6} {:<24} {:<30}",
                        row.info.provider.as_str(),
                        short(&session_id, 38),
                        fmt_age(row.info.updated_at_epoch_s),
                        short(row.info.title.as_deref().unwrap_or(""), 24),
                        short(&row.info.cwd, 30),
                    );
                }
            } else {
                for s in all.into_iter().filter(matches).take(limit) {
                    printed = printed.saturating_add(1);
                    println!(
                        "{:<8} {:<38} {:<6} {:<24} {:<30}",
                        s.provider.as_str(),
                        short(&s.session_id, 38),
                        fmt_age(s.updated_at_epoch_s),
                        short(s.title.as_deref().unwrap_or(""), 24),
                        short(&s.cwd, 30),
                    );
                }
            }
            debug_log(
                "session_ls_ok",
                serde_json::json!({
                    "printed": printed,
                    "tree": tree,
                }),
            );
        }

        SessionCmd::Show { id, full, json } => {
            debug_log(
                "session_show_start",
                serde_json::json!({
                    "id": &id,
                    "full": full,
                    "json": json,
                }),
            );
            let info = session::resolve(&id)?;
            debug_log(
                "session_show_resolved",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "cwd": &info.cwd,
                }),
            );
            let s = session::load(&info)?;
            debug_log(
                "session_show_loaded",
                serde_json::json!({
                    "session_id": &s.session_id,
                    "messages": s.messages.len(),
                }),
            );
            if json {
                let out = serde_json::to_string_pretty(&s)?;
                println!("{}", out);
                debug_log(
                    "session_show_ok",
                    serde_json::json!({
                        "mode": "json",
                        "bytes": out.len(),
                    }),
                );
            } else {
                let mode = if full {
                    session::render::Mode::Full
                } else {
                    session::render::Mode::Summary
                };
                let out = session::render::render(&s, mode);
                print!("{}", out);
                debug_log(
                    "session_show_ok",
                    serde_json::json!({
                        "mode": if full { "full" } else { "summary" },
                        "bytes": out.len(),
                    }),
                );
            }
        }

        SessionCmd::Clone {
            id,
            to,
            cwd,
            new_id,
            force,
        } => {
            debug_log(
                "clone_start",
                serde_json::json!({
                    "id": &id,
                    "to": to.map(provider_arg_label),
                    "cwd": cwd.as_deref(),
                    "new_id_provided": new_id.is_some(),
                    "force": force,
                }),
            );
            let info = session::resolve(&id)?;
            debug_log(
                "clone_resolved",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "cwd": &info.cwd,
                }),
            );
            let target = to.map(|p| p.to_lib()).transpose()?;
            if let Some(t) = target {
                if t != info.provider {
                    return Err(anyhow::anyhow!(
                        "cross-provider clone (--to {}) is not supported; clone preserves the source provider ({}). Use `cokacconvert convert` for cross-provider format conversion.",
                        t.as_str(),
                        info.provider.as_str()
                    ));
                }
            }
            debug_log(
                "clone_target",
                serde_json::json!({
                    "provider": target.as_ref().map(|provider| provider.as_str()),
                }),
            );
            let report = session::clone::clone_to_live(
                &info,
                &session::clone::CloneOpts {
                    to: target,
                    cwd,
                    overwrite: force,
                    new_id,
                },
            )?;
            let clone_tree_warning = session::clone_tree::record_clone_report(&report).err();
            debug_log(
                "clone_ok",
                serde_json::json!({
                    "source_provider": info.provider.as_str(),
                    "source_session_id": &info.session_id,
                    "target_provider": report.target_provider.as_str(),
                    "new_session_id": &report.new_session_id,
                    "artifact": format!("{:?}", &report.artifact),
                    "clone_tree_saved": clone_tree_warning.is_none(),
                }),
            );
            println!(
                "cloned {} ({}) → {} ({:?})\n  new session id: {}\n  artifact      : {:?}",
                info.session_id,
                info.provider.as_str(),
                report.target_provider.as_str(),
                report.target_provider,
                report.new_session_id,
                report.artifact,
            );
            if let Some(e) = clone_tree_warning {
                debug_log(
                    "clone_tree_warning",
                    serde_json::json!({
                        "error": e.to_string(),
                    }),
                );
                eprintln!("warning: clone tree metadata was not saved: {}", e);
            }
        }

        SessionCmd::Rm { id, yes } => {
            debug_log(
                "delete_start",
                serde_json::json!({
                    "id": &id,
                    "yes": yes,
                }),
            );
            let info = session::resolve(&id)?;
            debug_log(
                "delete_resolved",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "cwd": &info.cwd,
                }),
            );
            if !yes {
                debug_log(
                    "delete_confirm_prompt",
                    serde_json::json!({
                        "stdin_is_tty": std::io::stdin().is_terminal(),
                    }),
                );
                eprintln!(
                    "About to delete {} session {} ({})",
                    info.provider.as_str(),
                    info.session_id,
                    info.cwd
                );
                eprint!("Type 'yes' to confirm: ");
                std::io::stderr().flush().ok();
                let mut s = String::new();
                if std::io::stdin().is_terminal() {
                    std::io::stdin().read_line(&mut s)?;
                }
                if s.trim() != "yes" {
                    debug_log(
                        "delete_aborted",
                        serde_json::json!({
                            "session_id": &info.session_id,
                        }),
                    );
                    println!("aborted.");
                    debug_log("session_cmd_ok", serde_json::json!({}));
                    return Ok(());
                }
            }
            let report = session::remove::remove(&info)?;
            debug_log(
                "delete_ok",
                serde_json::json!({
                    "provider": report.provider.as_str(),
                    "session_id": &info.session_id,
                    "deleted_file": format!("{:?}", &report.deleted_file),
                    "deleted_rows": report.deleted_rows,
                }),
            );
            println!(
                "deleted {} session {}: file={:?} rows={}",
                report.provider.as_str(),
                info.session_id,
                report.deleted_file,
                report.deleted_rows
            );
        }

        SessionCmd::Search {
            query,
            case_sensitive,
            limit,
        } => {
            debug_log(
                "search_start",
                serde_json::json!({
                    "query_len": query.chars().count(),
                    "case_sensitive": case_sensitive,
                    "limit": limit,
                }),
            );
            let hits = session::search_all(&query, !case_sensitive)?;
            let hit_count = hits.len();
            println!(
                "{:<8} {:<38} {:<5} {}",
                "PROV", "SESSION_ID", "HITS", "SNIPPET"
            );
            let mut printed = 0usize;
            for h in hits.into_iter().take(limit) {
                printed = printed.saturating_add(1);
                println!(
                    "{:<8} {:<38} {:<5} {}",
                    h.info.provider.as_str(),
                    short(&h.info.session_id, 38),
                    h.matches,
                    short(&h.snippet, 100)
                );
            }
            debug_log(
                "search_ok",
                serde_json::json!({
                    "hits": hit_count,
                    "printed": printed,
                }),
            );
        }

        SessionCmd::Validate { id, json } => {
            debug_log(
                "session_validate_start",
                serde_json::json!({
                    "id": &id,
                    "json": json,
                }),
            );
            let info = session::resolve(&id)?;
            debug_log(
                "session_validate_resolved",
                serde_json::json!({
                    "provider": info.provider.as_str(),
                    "session_id": &info.session_id,
                    "source": info.source.display().to_string(),
                }),
            );
            let report = session::native_validate::validate_info(&info)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "{} {} native validation: {}",
                    report.provider.as_str(),
                    report.session_id,
                    if report.ok { "ok" } else { "failed" }
                );
                for check in &report.checks {
                    println!(
                        "  {} {} — {}",
                        if check.ok { "ok" } else { "FAIL" },
                        check.name,
                        check.detail
                    );
                }
            }
            debug_log(
                "session_validate_ok",
                serde_json::json!({
                    "provider": report.provider.as_str(),
                    "session_id": &report.session_id,
                    "ok": report.ok,
                    "checks": report.checks.len(),
                    "failures": report.checks.iter().filter(|check| !check.ok).count(),
                }),
            );
            if !report.ok {
                bail!(
                    "{} session {} failed native validation: {}",
                    report.provider.as_str(),
                    report.session_id,
                    report.failure_summary()
                );
            }
        }
    }
    debug_log("session_cmd_ok", serde_json::json!({}));
    Ok(())
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    debug_log(
        "cli_parsed",
        serde_json::json!({
            "cmd": cmd_label(&cli.cmd),
        }),
    );
    match cli.cmd {
        Cmd::Inspect { from, input, cwd } => {
            debug_log(
                "inspect_start",
                serde_json::json!({
                    "from": provider_arg_label(from),
                    "input": &input,
                    "cwd": cwd.as_deref(),
                }),
            );
            let prov = match from {
                ProviderArg::Auto => detect_provider(&input)?,
                other => other.to_lib()?,
            };
            debug_log(
                "inspect_provider",
                serde_json::json!({
                    "provider": prov.as_str(),
                }),
            );
            let src = parse_source(&input, prov)?;
            let mut session = read_session(prov, &src)?;
            debug_log(
                "inspect_read_ok",
                serde_json::json!({
                    "session_id": &session.session_id,
                    "messages": session.messages.len(),
                    "cwd_empty": session.cwd.is_empty(),
                }),
            );
            if let Some(c) = cwd {
                if session.cwd.is_empty() {
                    session.cwd = c;
                    debug_log(
                        "inspect_cwd_filled",
                        serde_json::json!({
                            "session_id": &session.session_id,
                        }),
                    );
                }
            }
            let s = serde_json::to_string_pretty(&session)?;
            println!("{}", s);
            debug_log(
                "inspect_ok",
                serde_json::json!({
                    "bytes": s.len(),
                }),
            );
        }
        Cmd::Convert {
            from,
            to,
            input,
            output,
        } => {
            debug_log(
                "convert_start",
                serde_json::json!({
                    "from": provider_arg_label(from),
                    "to": provider_arg_label(to),
                    "input": &input,
                    "output": &output,
                }),
            );
            let src_prov = match from {
                ProviderArg::Auto => detect_provider(&input)?,
                other => other.to_lib()?,
            };
            debug_log(
                "convert_source_provider",
                serde_json::json!({
                    "provider": src_prov.as_str(),
                }),
            );
            let src = parse_source(&input, src_prov)?;
            let session = read_session(src_prov, &src)?;
            debug_log(
                "convert_read_ok",
                serde_json::json!({
                    "session_id": &session.session_id,
                    "messages": session.messages.len(),
                }),
            );
            if to == ProviderArg::Universal {
                write_universal_to(&session, &output)?;
                debug_log(
                    "convert_ok",
                    serde_json::json!({
                        "target": "universal",
                        "messages": session.messages.len(),
                    }),
                );
                return Ok(());
            }
            let tgt_prov = to.to_lib()?;
            let dst = if tgt_prov == LibProvider::OpenCode {
                SessionTarget::OpenCodeDb {
                    db_path: PathBuf::from(&output),
                }
            } else {
                SessionTarget::Path(PathBuf::from(&output))
            };
            write_session(tgt_prov, &session, &dst)?;
            debug_log(
                "convert_ok",
                serde_json::json!({
                    "target": tgt_prov.as_str(),
                    "messages": session.messages.len(),
                    "output": &output,
                }),
            );
            eprintln!(
                "ok: {} messages → {} ({})",
                session.messages.len(),
                output,
                tgt_prov.as_str()
            );
        }
        Cmd::List {
            provider,
            cwd,
            limit,
        } => {
            debug_log(
                "list_start",
                serde_json::json!({
                    "provider": provider_arg_label(provider),
                    "cwd": cwd.as_deref(),
                    "limit": limit,
                }),
            );
            let p = provider.to_lib()?;
            let mut items = providers::discovery::list_all(p)?;
            debug_log(
                "list_loaded",
                serde_json::json!({
                    "provider": p.as_str(),
                    "total": items.len(),
                }),
            );
            if let Some(c) = cwd {
                items.retain(|i| i.cwd == c);
                debug_log(
                    "list_filtered",
                    serde_json::json!({
                        "remaining": items.len(),
                    }),
                );
            }
            items.truncate(limit);
            let printed = items.len();
            println!(
                "{:<8} {:<38} {:<10} {}",
                "PROV", "SESSION_ID", "MTIME", "CWD"
            );
            for it in items {
                println!(
                    "{:<8} {:<38} {:<10} {}",
                    it.provider.as_str(),
                    it.session_id,
                    it.updated_at_epoch_s,
                    it.cwd
                );
            }
            debug_log(
                "list_ok",
                serde_json::json!({
                    "printed": printed,
                }),
            );
        }
        Cmd::Session { cmd } => {
            run_session(cmd)?;
        }
    }
    Ok(())
}

fn main() {
    init_debug_from_env_or_settings();
    debug_log(
        "main_start",
        serde_json::json!({
            "pid": std::process::id(),
        }),
    );
    match run() {
        Ok(()) => {
            debug_log("main_ok", serde_json::json!({}));
        }
        Err(e) => {
            debug_log(
                "main_error",
                serde_json::json!({
                    "error": e.to_string(),
                }),
            );
            eprintln!("error: {:#}", e);
            std::process::exit(1);
        }
    }
}

fn tree_indent(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!("{}└ ", "  ".repeat(depth.saturating_sub(1)))
    }
}
