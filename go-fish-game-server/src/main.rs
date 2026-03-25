use clap::Parser;
use go_fish_game_server::{run, Config};
use opentelemetry::global;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::warn;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(name = "go-fish-game-server")]
struct Cli {
    /// Path to a TOML config file
    #[arg(long)]
    config: Option<std::path::PathBuf>,
}

fn init_tracer_provider() -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
        .expect("failed to build OTLP span exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(opentelemetry_sdk::Resource::builder()
            .with_service_name("go-fish-game-server")
            .build())
        .build();

    global::set_tracer_provider(provider.clone());
    provider
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let provider = init_tracer_provider();
    let otel_layer = tracing_opentelemetry::layer().with_tracer(global::tracer("go-fish-game-server"));

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::fmt::layer())
        .with(otel_layer)
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

    run(config).await?;

    // Flush spans before exit
    let _ = provider.shutdown();
    Ok(())
}
