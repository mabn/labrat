use clap::Parser;
use labrat_core::{App, Config, Db, Edition};
use std::sync::Arc;

/// A GitLab API simulator for testing integrations.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Address to listen on.
    #[arg(long, env = "LABRAT_LISTEN", default_value = "127.0.0.1:8080")]
    listen: String,
    /// SQLite database file, or ":memory:".
    #[arg(long, env = "LABRAT_DB", default_value = "labrat.sqlite")]
    db: String,
    /// Base URL used in links (defaults to http://<listen>).
    #[arg(long, env = "LABRAT_EXTERNAL_URL")]
    external_url: Option<String>,
    /// Password of the root user created on an empty database.
    #[arg(
        long,
        env = "LABRAT_ROOT_PASSWORD",
        default_value = "labrat-root-password"
    )]
    root_password: String,
    /// Give root a personal access token with this exact value.
    #[arg(long, env = "LABRAT_ROOT_TOKEN")]
    root_token: Option<String>,
    /// Port shown in ssh_url_to_repo.
    #[arg(long, env = "LABRAT_SSH_PORT", default_value_t = 22)]
    ssh_port: u16,
    /// Delay for every background job, in milliseconds.
    #[arg(long, env = "LABRAT_JOB_DELAY_MS", default_value_t = 0)]
    job_delay_ms: u64,
    /// Random extra delay of up to this many milliseconds per job.
    #[arg(long, env = "LABRAT_JOB_JITTER_MS", default_value_t = 0)]
    job_jitter_ms: u64,
    /// Behave like GitLab Community Edition: EE-only endpoints and fields (approval rules and
    /// settings, among others) answer exactly as on CE. The default is Enterprise Edition.
    #[arg(long, alias = "no-ee", env = "LABRAT_NOEE")]
    noee: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let external_url = args
        .external_url
        .unwrap_or_else(|| format!("http://{}", args.listen));
    let external_url = external_url.trim_end_matches('/').to_string();
    let host = url::Url::parse(&external_url)?
        .host_str()
        .unwrap_or("localhost")
        .to_string();
    let config = Config {
        external_url,
        ssh_host: host,
        ssh_port: args.ssh_port,
        root_password: args.root_password,
        root_token: args.root_token,
        edition: if args.noee { Edition::Ce } else { Edition::Ee },
    };
    let app = Arc::new(App {
        db: Db::open(&args.db)?,
        config,
    });
    labrat_api::bootstrap(&app)?;
    labrat_core::jobs::set_delays(labrat_core::jobs::Delays {
        default_ms: args.job_delay_ms as i64,
        jitter_ms: args.job_jitter_ms as i64,
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind(&args.listen).await?;
    tracing::info!(
        "labrat listening on {} ({})",
        args.listen,
        app.config.external_url
    );
    axum::serve(listener, labrat_api::router(app)).await?;
    Ok(())
}
