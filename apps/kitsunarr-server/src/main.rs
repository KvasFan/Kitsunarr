//! Kitsunarr — a modern Rust-powered media manager for TV, anime and movies.

mod assets;
mod jobs;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use kitsunarr_api::{AppState, BuildInfo};
use kitsunarr_config::{Config, LogFormat, ProcessEnv};
use kitsunarr_db::Database;
use kitsunarr_domain::api::migration::{
    ApiSourceRequest, ConflictPolicy, MigrationOptions, PathMapping,
};
use kitsunarr_events::EventBus;
use tokio_util::sync::CancellationToken;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "kitsunarr",
    version,
    about = "A modern Rust-powered media manager for TV, anime and movies."
)]
struct Cli {
    /// Directory containing config.toml [env: KITSUNARR_CONFIG_DIR] [default: /config]
    #[arg(long, global = true)]
    config_dir: Option<PathBuf>,
    /// Directory for the database, backups and temporary files [env: KITSUNARR_DATA_DIR]
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// HTTP port [env: KITSUNARR_PORT]
    #[arg(long, global = true)]
    port: Option<u16>,
    /// Bind address [env: KITSUNARR_BIND_ADDRESS]
    #[arg(long, global = true)]
    bind: Option<std::net::IpAddr>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (default).
    Serve,
    /// Probe a running server (exit code 0 when healthy); used by the container health check.
    Healthcheck,
    /// Write a consistent copy of the database.
    Backup {
        /// Destination file (default: <data-dir>/backups/kitsunarr_<timestamp>_manual.db)
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Replace the database with a backup. The server must be stopped.
    Restore {
        file: PathBuf,
        /// Skip the check that no server is answering on the configured port.
        #[arg(long)]
        force: bool,
    },
    /// Migrate from Sonarr or Tanukarr without the web interface (dry run unless --apply).
    Migrate {
        /// Backup archive (.zip) or sonarr.db file.
        #[arg(long, conflicts_with = "url")]
        backup: Option<PathBuf>,
        /// Base URL of a running Sonarr/Tanukarr instance.
        #[arg(long, requires = "api_key")]
        url: Option<String>,
        #[arg(long, env = "KITSUNARR_SOURCE_API_KEY", hide_env_values = true)]
        api_key: Option<String>,
        /// Path mapping SOURCE=TARGET (repeatable), e.g. --map /tv=/media/tv
        #[arg(long = "map", value_parser = parse_mapping)]
        mappings: Vec<PathMapping>,
        /// Entity groups to import (comma separated); default all.
        #[arg(long, value_delimiter = ',')]
        entities: Vec<String>,
        /// Overwrite existing Kitsunarr records instead of skipping them.
        #[arg(long)]
        update: bool,
        #[arg(long)]
        no_history: bool,
        #[arg(long)]
        no_verify_paths: bool,
        /// Apply the migration (otherwise only a dry run is performed).
        #[arg(long)]
        apply: bool,
    },
    /// Print the effective configuration.
    Config,
}

fn parse_mapping(s: &str) -> Result<PathMapping, String> {
    let (src, dst) = s.split_once('=').ok_or("expected SOURCE=TARGET")?;
    Ok(PathMapping {
        source_prefix: src.into(),
        target_prefix: dst.into(),
    })
}

fn load_config(cli: &Cli) -> Result<Config> {
    let mut config = Config::load(cli.config_dir.as_deref(), &ProcessEnv)?;
    if let Some(d) = &cli.data_dir {
        config.data_dir = d.clone();
    }
    if let Some(p) = cli.port {
        config.server.port = p;
    }
    if let Some(b) = cli.bind {
        config.server.bind_address = b;
    }
    config.normalize()?;
    Ok(config)
}

fn init_logging(config: &Config, logs: &kitsunarr_api::LogBuffer) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = EnvFilter::try_from_env("KITSUNARR_LOG").unwrap_or_else(|_| {
        EnvFilter::try_new(format!(
            "{},tower_http=warn,sqlx=warn,hyper=warn",
            config.log.level
        ))
        .unwrap_or_else(|_| EnvFilter::new("info"))
    });
    let json = config.log.format == LogFormat::Json;
    tracing_subscriber::registry()
        .with(filter)
        .with(json.then(|| tracing_subscriber::fmt::layer().json().flatten_event(true)))
        .with((!json).then(|| tracing_subscriber::fmt::layer().with_target(true)))
        .with(logs.layer())
        .init();
}

fn ensure_writable(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot create data directory {}", dir.display()))?;
    let probe = dir.join(".kitsunarr-write-test");
    std::fs::write(&probe, b"ok").with_context(|| {
        format!(
            "data directory {} is not writable; set KITSUNARR_DATA_DIR or --data-dir to a writable location",
            dir.display()
        )
    })?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

async fn open_database(config: &Config) -> Result<Database> {
    ensure_writable(&config.data_dir)?;
    let db = Database::open(&config.database_path())
        .await
        .with_context(|| format!("cannot open database {}", config.database_path().display()))?;
    kitsunarr_db::seed::seed_defaults(&db).await?;
    Ok(db)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("kitsunarr: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    let config = load_config(&cli)?;
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config).await,
        Command::Healthcheck => healthcheck(&config).await,
        Command::Backup { output } => {
            let db = open_database(&config).await?;
            let dest = output.unwrap_or_else(|| {
                kitsunarr_db::maintenance::unique_backup_path(&config.backup_dir(), "manual")
            });
            let path = kitsunarr_db::maintenance::backup_to(&db, &dest).await?;
            println!("{}", path.display());
            Ok(())
        }
        Command::Restore { file, force } => restore(&config, &file, force).await,
        Command::Migrate {
            backup,
            url,
            api_key,
            mappings,
            entities,
            update,
            no_history,
            no_verify_paths,
            apply,
        } => {
            let options = MigrationOptions {
                path_mappings: mappings,
                entities,
                conflict_policy: if update {
                    ConflictPolicy::Update
                } else {
                    ConflictPolicy::Skip
                },
                include_history: !no_history,
                verify_paths: !no_verify_paths,
            };
            migrate_cli(&config, backup, url, api_key, options, apply).await
        }
        Command::Config => {
            println!(
                "# config dir: {}\n# database:   {}",
                config.config_dir.display(),
                config.database_path().display()
            );
            println!("{}", toml_like(&config));
            Ok(())
        }
    }
}

fn toml_like(config: &Config) -> String {
    format!(
        "data_dir = {:?}\n\n[server]\nbind_address = \"{}\"\nport = {}\nurl_base = {:?}\ntrusted_proxies = {:?}\n\n[auth]\nsession_days = {}\nsecure_cookies = {:?}\n\n[log]\nlevel = {:?}\nformat = {:?}",
        config.data_dir.display().to_string(),
        config.server.bind_address,
        config.server.port,
        config.server.url_base,
        config
            .server
            .trusted_proxies
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        config.auth.session_days,
        format!("{:?}", config.auth.secure_cookies).to_lowercase(),
        config.log.level,
        format!("{:?}", config.log.format).to_lowercase(),
    )
}

async fn shutdown_signal(token: CancellationToken) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
        _ = token.cancelled() => {},
    }
    tracing::info!("shutdown requested");
    token.cancel();
}

async fn serve(config: Config) -> Result<()> {
    let logs = kitsunarr_api::LogBuffer::default();
    init_logging(&config, &logs);
    tracing::info!(version = VERSION, data_dir = %config.data_dir.display(), "starting Kitsunarr");
    let db = open_database(&config).await?;
    if let Some(dir) = Some(&config.config_dir).filter(|d| d.is_dir()) {
        let file = dir.join(kitsunarr_config::CONFIG_FILE_NAME);
        if !file.exists() && std::fs::write(&file, Config::example_file()).is_ok() {
            tracing::info!(file = %file.display(), "wrote example configuration");
        }
    }
    let api_key = kitsunarr_db::seed::ensure_api_key(&db, kitsunarr_auth::generate_api_key).await?;
    let interrupted = kitsunarr_db::migration_runs::fail_interrupted(db.pool()).await?;
    if interrupted > 0 {
        tracing::warn!(
            interrupted,
            "migration runs interrupted by the previous shutdown were rolled back"
        );
    }
    if kitsunarr_db::auth::user_count(db.pool()).await? == 0 {
        tracing::warn!("no administrator account exists yet: open the web interface to create one");
    }

    let events = EventBus::default();
    let shutdown = CancellationToken::new();
    let tmp_dir = config.data_dir.join("tmp");
    // Every outgoing request consults the proxy settings, so saving them applies at once.
    let proxy = kitsunarr_api::proxy::HttpProxy::default();
    match kitsunarr_db::settings::get_or_default::<kitsunarr_domain::api::settings::ProxySettings>(
        db.pool(),
        kitsunarr_db::settings::KEY_PROXY,
    )
    .await
    {
        Ok(s) => proxy.set(s),
        Err(e) => tracing::warn!(error = %e, "could not load the proxy settings"),
    }
    // Sonarr's certificate validation, consulted by every HTTPS connection.
    let certificates = kitsunarr_api::tls::CertificatePolicy::default();
    match kitsunarr_db::settings::get_or_default::<kitsunarr_domain::api::settings::GeneralSettings>(
        db.pool(),
        kitsunarr_db::settings::KEY_GENERAL,
    )
    .await
    {
        Ok(g) => certificates.set(g.certificate_validation),
        Err(e) => tracing::warn!(error = %e, "could not load the general settings"),
    }
    let mut http = reqwest::Client::builder();
    if let Some(config) = certificates.rustls_config() {
        http = http.tls_backend_preconfigured(config);
    }
    let http = http
        .proxy(proxy.reqwest())
        .user_agent(format!(
            "Kitsunarr/{VERSION} (+https://github.com/KvasFan/Kitsunarr)"
        ))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()?;
    let metadata = kitsunarr_metadata::Metadata::with(
        http.clone(),
        &kitsunarr_metadata::Endpoints {
            tvmaze: config.metadata.tvmaze_url.clone(),
            anilist: config.metadata.anilist_url.clone(),
            tvdb: config.metadata.tvdb_url.clone(),
            tmdb: config.metadata.tmdb_url.clone(),
            jikan: config.metadata.jikan_url.clone(),
            omdb: config.metadata.omdb_url.clone(),
            xem: config.metadata.xem_url.clone(),
        },
    );
    let downloads = kitsunarr_library::downloads::Monitor::new(http.clone());
    // Jobs that need the API's state (RSS sync) receive it once it exists.
    let api_state: Arc<std::sync::OnceLock<AppState>> = Arc::default();
    let scheduler = kitsunarr_scheduler::Scheduler::new(
        db.clone(),
        events.clone(),
        jobs::all(
            config.backup_dir(),
            tmp_dir.join("migration"),
            metadata.clone(),
            downloads.clone(),
            api_state.clone(),
        ),
        shutdown.child_token(),
    );

    let config = Arc::new(config);
    // Metadata consumers download artwork through the same (proxy-aware) client.
    kitsunarr_library::metadata_files::install_http_client(http.clone());
    let state = AppState::new(
        db.clone(),
        config.clone(),
        events,
        api_key,
        BuildInfo {
            version: VERSION,
            git_revision: option_env!("KITSUNARR_GIT_REVISION").filter(|r| !r.is_empty()),
        },
        Arc::new(assets::Embedded),
        Arc::new(scheduler.clone()),
        http,
        metadata,
        logs,
    )
    .with_download_monitor(downloads)
    .with_proxy(proxy)
    .with_certificates(certificates);
    let _ = api_state.set(state.clone());
    // `ApplicationUpdatedEvent`: the first start of a new version.
    kitsunarr_library::notify::announce_version(&db, &state.downloads.notifier, VERSION).await;
    if let Err(e) = kitsunarr_api::routes::settings::apply_metadata_keys(&state).await {
        tracing::warn!(error = %e.message(), "could not load the metadata catalogue keys");
    }
    scheduler.start(Duration::from_secs(30)).await?;
    kitsunarr_api::rss::sync_interval(&db).await?;
    if !assets::Embedded::has_ui() {
        tracing::warn!("this build does not include the web interface; only the API is served");
    }

    let addr = SocketAddr::new(config.server.bind_address, config.server.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("cannot listen on {addr}"))?;
    tracing::info!("listening on http://{addr}{}/", config.server.url_base);
    let app = kitsunarr_api::app(state);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(shutdown.clone()))
    .await?;

    tracing::info!("stopping background tasks");
    scheduler.shutdown(Duration::from_secs(30)).await;
    db.close().await;
    tracing::info!("Kitsunarr stopped");
    Ok(())
}

async fn healthcheck(config: &Config) -> Result<()> {
    let host = match config.server.bind_address {
        std::net::IpAddr::V4(v4) if v4.is_unspecified() => "127.0.0.1".to_string(),
        std::net::IpAddr::V6(v6) if v6.is_unspecified() => "[::1]".to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        ip => ip.to_string(),
    };
    let url = format!(
        "http://{host}:{}{}/api/v1/ping",
        config.server.port, config.server.url_base
    );
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(&url)
        .send()
        .await
        .with_context(|| format!("{url} unreachable"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    Ok(())
}

async fn restore(config: &Config, file: &Path, force: bool) -> Result<()> {
    if !force && healthcheck(config).await.is_ok() {
        bail!(
            "a Kitsunarr server is answering on port {}; stop it before restoring (or pass --force)",
            config.server.port
        );
    }
    // Validate the backup by opening it (applying pending migrations to a scratch copy).
    let scratch = tempfile::tempdir()?;
    let probe = scratch.path().join("probe.db");
    std::fs::copy(file, &probe).with_context(|| format!("cannot read {}", file.display()))?;
    let test = Database::open(&probe)
        .await
        .context("the file is not a usable Kitsunarr database")?;
    let report = kitsunarr_db::maintenance::integrity_check(&test).await?;
    test.close().await;
    if !report.ok() {
        bail!("the backup failed its integrity check: {report:?}");
    }
    let target = config.database_path();
    if target.exists() {
        let aside = target.with_extension(format!(
            "db.pre-restore-{}",
            chrono::Utc::now().format("%Y%m%d%H%M%S")
        ));
        std::fs::rename(&target, &aside)?;
        println!("previous database kept as {}", aside.display());
    }
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(target.with_extension(ext));
    }
    std::fs::copy(file, &target)?;
    println!("restored {} → {}", file.display(), target.display());
    Ok(())
}

async fn migrate_cli(
    config: &Config,
    backup: Option<PathBuf>,
    url: Option<String>,
    api_key: Option<String>,
    options: MigrationOptions,
    apply: bool,
) -> Result<()> {
    let db = open_database(config).await?;
    let progress = |msg: String, done: u64, total: u64| eprintln!("  [{done}/{total}] {msg}");
    let source = match (backup, url) {
        (Some(file), _) => {
            kitsunarr_migrate::load_from_backup(
                &file,
                &config.data_dir.join("tmp").join("migration"),
            )
            .await?
        }
        (None, Some(url)) => {
            let req = ApiSourceRequest {
                url,
                api_key: api_key.unwrap_or_default(),
                allow_private_network: true,
            };
            kitsunarr_migrate::load_from_api(
                &req,
                &format!("Kitsunarr/{VERSION} (migration)"),
                &progress,
            )
            .await?
        }
        (None, None) => bail!("pass --backup FILE or --url URL --api-key KEY"),
    };
    let d = &source.detected;
    println!(
        "Source: {:?} {} (schema {}) via {:?}{}",
        d.app,
        d.product_version.as_deref().unwrap_or(&d.app_version),
        d.schema_version.map_or("?".into(), |v| v.to_string()),
        d.method,
        d.instance_name
            .as_ref()
            .map(|n| format!(", instance \"{n}\""))
            .unwrap_or_default()
    );
    for (entity, count) in source.snapshot.inventory() {
        if count > 0 {
            println!("  {entity:<22} {count}");
        }
    }
    let stage = |stage: &str, done: u64, total: u64| eprintln!("  {stage}: {done}/{total}");
    let mode = if apply {
        kitsunarr_migrate::Mode::Apply {
            backup_dir: config.backup_dir(),
        }
    } else {
        kitsunarr_migrate::Mode::DryRun
    };
    let outcome = kitsunarr_migrate::run(&db, &source, &options, mode, &stage).await?;
    println!(
        "\n{:<22} {:>7} {:>7} {:>7} {:>11}",
        "entity", "create", "update", "skip", "unsupported"
    );
    for s in &outcome.summary {
        println!(
            "{:<22} {:>7} {:>7} {:>7} {:>11}",
            s.entity, s.create, s.update, s.skip, s.unsupported
        );
    }
    for w in &outcome.warnings {
        println!(
            "[{:?}] {}{}: {}",
            w.severity,
            w.entity,
            w.title
                .as_ref()
                .map(|t| format!(" \"{t}\""))
                .unwrap_or_default(),
            w.message
        );
    }
    for c in &outcome.validation {
        println!(
            "check {:<26} {} (expected {}, actual {})",
            c.name,
            if c.passed { "ok" } else { "FAILED" },
            c.expected,
            c.actual
        );
    }
    for b in &outcome.blocking {
        println!("BLOCKING: {b}");
    }
    if apply {
        println!(
            "\nApplied (run #{}). Database backup: {}",
            outcome.run_id.unwrap_or_default(),
            outcome.backup_path.unwrap_or_default()
        );
    } else {
        println!("\nDry run only: nothing was changed. Re-run with --apply to import.");
    }
    Ok(())
}
