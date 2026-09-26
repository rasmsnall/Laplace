mod access;
mod alerts;
mod anomaly;
mod api;
mod checks;
mod config;
mod databricks;
mod db;
mod export;
mod guard;
mod holidays;
mod human;
mod inbound;
mod jobs;
mod mail;
mod ops;
mod pipeline;
mod registry;
mod reports;
mod schedule;
mod settings;
mod settings_api;
mod sso;
mod status;
mod timeline;
mod tokens;

use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};
use axum::http::{HeaderValue, header::CACHE_CONTROL};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeader;

use config::{Config, Flow};
use inbound::CallBuffer;
use settings::Settings;

pub struct App {
    /// swapped when flows.toml changes, so each use takes a snapshot
    config: RwLock<Arc<Config>>,
    config_path: String,
    /// the file as last read, to notice changes; kept even when it did not parse
    config_text: Mutex<String>,
    /// why the file on disk is not the one in use
    pub config_error: RwLock<Option<String>>,
    pub pool: sqlx::PgPool,
    pub client: reqwest::Client,
    pub calls: CallBuffer,
    pub alert_webhook: Option<String>,
    pub mailer: Option<mail::Mailer>,
    /// ALERT_EMAIL, for flows whose owner has no addresses
    pub alert_email: Vec<String>,
    /// the token from LAPLACE_TOKEN; tokens made in the dashboard are checked through `tokens`
    pub ingest_token: Option<String>,
    pub tokens: tokens::Cache,
    pub limiter: guard::Limiter,
    /// whether a proxy in front sets x-forwarded-for, so rate limits see the real client
    pub trust_proxy: bool,
    /// the address jobs use to reach us, when it differs from the one the dashboard is opened on
    pub public_url: Option<String>,
    pub delta_uri: Option<String>,
    /// sign-in, when configured
    pub sso: Option<sso::Sso>,
    /// emails from LAPLACE_ADMINS, always admins, so nobody can lock everyone out
    pub admins: Vec<String>,
    /// what the environment says; an admin's changes in the dashboard go on top
    pub default_settings: Settings,
    settings: RwLock<Settings>,
    /// when this replica's scheduler last ticked, in unix seconds; the liveness probe reads it
    pub last_tick: AtomicI64,
    /// an outside service laplace pings every minute; it alerts when the pings stop
    pub heartbeat_url: Option<String>,
}

impl App {
    /// flows from the config file, plus flows jobs registered by reporting in; the file wins on a clash
    pub fn config(&self) -> Arc<Config> {
        self.config.read().unwrap().clone()
    }

    /// picks up an edited flows.toml; a file that does not parse is reported and the old one kept
    pub async fn reload_config(&self) -> Result<()> {
        let text = tokio::fs::read_to_string(&self.config_path).await?;
        if *self.config_text.lock().unwrap() == text {
            return Ok(());
        }
        *self.config_text.lock().unwrap() = text.clone();
        match Config::parse(&text) {
            Ok(config) => {
                let ids: Vec<String> = config.flows.iter().map(|flow| flow.id.clone()).collect();
                db::ensure_flows(&self.pool, &ids).await?;
                println!("{} reloaded: {} flows", self.config_path, ids.len());
                *self.config.write().unwrap() = Arc::new(config);
                *self.config_error.write().unwrap() = None;
            }
            Err(error) => {
                let error = format!(
                    "{} not loaded, the previous version is in use: {error:#}",
                    self.config_path
                );
                eprintln!("{error}");
                *self.config_error.write().unwrap() = Some(error);
            }
        }
        Ok(())
    }

    /// every flow's status, with maintenance windows applied
    pub async fn statuses(&self) -> Result<Vec<status::Status>> {
        let zone = self.settings().timezone;
        let mut statuses = status::all(&self.pool, &self.flows().await?, zone).await?;
        status::apply_maintenance(&mut statuses, &self.config(), zone, chrono::Utc::now());
        Ok(statuses)
    }

    pub async fn flows(&self) -> Result<Vec<Flow>> {
        let config = self.config();
        let mut flows = config.flows.clone();
        let registered = registry::all(&self.pool).await?;
        flows.extend(
            registered
                .into_iter()
                .filter(|flow| config.flow(&flow.id).is_none()),
        );
        Ok(flows)
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub async fn reload_settings(&self) -> Result<()> {
        let fresh = Settings::load(&self.pool, &self.default_settings).await?;
        *self.settings.write().unwrap() = fresh;
        Ok(())
    }

    pub async fn flow(&self, id: &str) -> Result<Option<Flow>> {
        match self.config().flow(id) {
            Some(flow) => Ok(Some(flow.clone())),
            None => registry::find(&self.pool, id).await,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some("check") => check(args.get(1).map_or("flows.toml", String::as_str)),
        Some(other) => {
            anyhow::bail!("unknown command {other}: use serve, migrate or check <flows.toml>")
        }
    }
}

/// DATABASE_URL, or the standard PGHOST, PGUSER, PGPASSWORD, PGDATABASE and PGSSLMODE,
/// which let a password come straight from a secret an operator made
async fn connect(max_connections: u32) -> Result<sqlx::PgPool> {
    let options = match optional_env("DATABASE_URL") {
        Some(url) => url
            .parse()
            .context("DATABASE_URL is not a valid postgres url")?,
        None if optional_env("PGHOST").is_some() => PgConnectOptions::new(),
        None => anyhow::bail!("set DATABASE_URL, or PGHOST and the other PG variables"),
    };
    Ok(PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?)
}

/// runs the migrations and exits; in kubernetes a job does this with the schema owner's
/// credentials, so the running service never needs the right to change tables
async fn migrate() -> Result<()> {
    sqlx::migrate!().run(&connect(1).await?).await?;
    println!("migrations applied");
    Ok(())
}

/// validates a flows file without a database, for ci before a change is merged
fn check(path: &str) -> Result<()> {
    let config = Config::load(path)?;
    println!(
        "{path}: {} flows, {} owners, {} maintenance windows, ok",
        config.flows.len(),
        config.owners.len(),
        config.maintenance.len()
    );
    Ok(())
}

async fn serve() -> Result<()> {
    deltalake::azure::register_handlers(None);
    deltalake::gcp::register_handlers(None);

    let config_path = env_or("LAPLACE_CONFIG", "flows.toml");
    let config_text =
        std::fs::read_to_string(&config_path).with_context(|| format!("reading {config_path}"))?;
    let config = Config::parse(&config_text).with_context(|| format!("in {config_path}"))?;
    let pool = connect(10).await?;
    if env_or("RUN_MIGRATIONS", "true") != "false" {
        sqlx::migrate!().run(&pool).await?;
    }

    let public_url =
        optional_env("LAPLACE_PUBLIC_URL").map(|url| url.trim_end_matches('/').to_owned());
    let default_settings = Settings {
        timezone: env_or("LAPLACE_TIMEZONE", "Europe/Stockholm")
            .parse()
            .map_err(|_| anyhow::anyhow!("LAPLACE_TIMEZONE is not a known time zone"))?,
        retention_days: env_or("RETENTION_DAYS", "30")
            .parse()
            .context("RETENTION_DAYS")?,
        auto_register: env_or("AUTO_REGISTER", "true") != "false",
    };
    let settings = Settings::load(&pool, &default_settings).await?;
    let client = reqwest::Client::builder().tls_info(true).build()?;
    let mailer = mail::Mailer::from_env(&client)?;
    let app = Arc::new(App {
        config: RwLock::new(Arc::new(config)),
        config_path,
        config_text: Mutex::new(config_text),
        config_error: RwLock::default(),
        pool,
        client,
        calls: CallBuffer::default(),
        alert_webhook: optional_env("ALERT_WEBHOOK"),
        mailer,
        alert_email: mail::shared_recipients()?,
        ingest_token: optional_env("LAPLACE_TOKEN"),
        tokens: tokens::Cache::default(),
        limiter: guard::Limiter::default(),
        trust_proxy: env_or("LAPLACE_TRUST_PROXY", "false") == "true",
        public_url: public_url.clone(),
        sso: sso::Sso::from_env(public_url.as_deref())?,
        admins: optional_env("LAPLACE_ADMINS")
            .map(|admins| {
                admins
                    .split(',')
                    .map(|email| email.trim().to_lowercase())
                    .filter(|e| !e.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        delta_uri: optional_env("DELTA_URI"),
        default_settings,
        settings: RwLock::new(settings),
        last_tick: AtomicI64::new(chrono::Utc::now().timestamp()),
        heartbeat_url: optional_env("LAPLACE_HEARTBEAT_URL"),
    });
    if app.sso.is_some() && app.admins.is_empty() {
        println!(
            "warning: sign-in is on but LAPLACE_ADMINS is empty, so only people added in the database get in"
        );
    }
    let wants_email = !app.alert_email.is_empty()
        || app
            .config()
            .owners
            .values()
            .any(|owner| !owner.email.is_empty());
    if wants_email && app.mailer.is_none() {
        println!(
            "warning: alerts have email addresses but LAPLACE_SMTP_URL is not set, so no email is sent"
        );
    }

    let flow_ids: Vec<String> = app
        .config()
        .flows
        .iter()
        .map(|flow| flow.id.clone())
        .collect();
    db::ensure_flows(&app.pool, &flow_ids).await?;
    db::sync_jobs(
        &app.pool,
        &jobs::intervals(&app).await?.into_keys().collect::<Vec<_>>(),
    )
    .await?;
    let scheduler = tokio::spawn(jobs::run_forever(app.clone()));

    let web = env_or("WEB_DIR", "web/dist");
    let signed_in = api::signed_in_routes(app.clone())
        .merge(settings_api::routes(app.clone()))
        .nest_service("/assets", web_assets(&web))
        .fallback_service(web_pages(&web))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            access::identify,
        ))
        .layer(axum::middleware::from_fn(guard::same_origin_writes));
    if app.sso.is_some() {
        println!("sign-in required");
    }
    let open = api::open_routes(app.clone())
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            guard::limit_open,
        ))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_REPORT_BYTES));
    let router = open
        .merge(signed_in)
        .layer(axum::middleware::from_fn(guard::security_headers));

    let port = env_or("PORT", "8090");
    let ops_port = env_or("LAPLACE_OPS_PORT", "9090");
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    let ops_listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{ops_port}")).await?;
    println!("laplace listening on :{port}, probes and metrics on :{ops_port}");

    let stopping = tokio_util::sync::CancellationToken::new();
    let ops = axum::serve(ops_listener, ops::routes(app.clone()))
        .with_graceful_shutdown(stopping.clone().cancelled_owned());
    let web = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stopping.clone().cancelled_owned());
    tokio::spawn({
        let stopping = stopping.clone();
        async move {
            shutdown_signal().await;
            println!("stopping: finishing requests");
            stopping.cancel();
        }
    });
    tokio::try_join!(web.into_future(), ops.into_future())?;

    // the scheduler would otherwise take counts from the call buffer into a dying process
    scheduler.abort();
    app.calls.flush(&app.pool).await?;
    println!("stopped cleanly");
    Ok(())
}

/// kubernetes sends sigterm before it removes a pod; ctrl-c when running by hand
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

/// a job report or a batch of calls; far above what a real one needs
const MAX_REPORT_BYTES: usize = 1024 * 1024;

/// built files carry a content hash in their name, so they can be cached for good
fn web_assets(web: &str) -> SetResponseHeader<ServeDir, HeaderValue> {
    let forever = HeaderValue::from_static("public, max-age=31536000, immutable");
    SetResponseHeader::overriding(
        ServeDir::new(format!("{web}/assets")),
        CACHE_CONTROL,
        forever,
    )
}

/// the page itself is checked on every load, so a new version shows up right after a deploy
fn web_pages(web: &str) -> SetResponseHeader<ServeDir<ServeFile>, HeaderValue> {
    let pages = ServeDir::new(web).fallback(ServeFile::new(format!("{web}/index.html")));
    SetResponseHeader::overriding(pages, CACHE_CONTROL, HeaderValue::from_static("no-cache"))
}

/// compose and kubernetes pass unset variables as empty strings
pub fn optional_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}
