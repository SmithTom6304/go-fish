use clap::Parser;
use go_fish_game_server::{run, Config};
use opentelemetry::{global, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::{logs::SdkLoggerProvider, trace::SdkTracerProvider};
use tracing::warn;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "go-fish-game-server")]
struct Cli {
    /// Path to a TOML config file
    #[arg(long)]
    config: Option<std::path::PathBuf>,
}

fn service_resource() -> opentelemetry_sdk::Resource {
    opentelemetry_sdk::Resource::builder()
        .with_service_name("go-fish-game-server")
        .with_attribute(opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build()
}

fn init_tracer_provider() -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
        .expect("failed to build OTLP span exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(service_resource())
        .build();

    global::set_tracer_provider(provider.clone());
    provider
}

fn init_logger_provider() -> SdkLoggerProvider {
    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .build()
        .expect("failed to build OTLP log exporter");

    SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(service_resource())
        .build()
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let tracer_provider = init_tracer_provider();
    let logger_provider = init_logger_provider();

    let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("go-fish-game-server"));
    let log_layer = OpenTelemetryTracingBridge::new(&logger_provider);

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .with(trace_layer)
        .with(log_layer)
        .init();

    let cli = Cli::parse();

    let config = match cli.config {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(contents) => match toml::from_str::<Config>(&contents) {
                Ok(cfg) => cfg,
                Err(e) => {
                    warn!(error = %e, path = %path.display(), "Failed to parse config file, using defaults");
                    Config::default()
                }
            },
            Err(e) => {
                warn!(error = %e, path = %path.display(), "Failed to read config file, using defaults");
                Config::default()
            }
        },
        None => Config::default(),
    };

    tracing::info!(config = ?config, "Starting go-fish-game-server");
    run(config).await?;

    // Flush telemetry before exit
    let _ = tracer_provider.shutdown();
    let _ = logger_provider.shutdown();
    Ok(())
}
