use tracing::*;
use tracing_subscriber::{prelude::*, EnvFilter};

pub fn log_init(#[cfg(feature = "tracing-journald")] journald: bool) {
    let subscriber = tracing_subscriber::registry();

    // Respect RUST_LOG, falling back to INFO
    let filter = EnvFilter::builder()
        .with_default_directive(Level::INFO.into())
        .from_env_lossy();
    let subscriber = subscriber.with(filter);

    #[cfg(feature = "tracing-journald")]
    if opts.journald {
        let subscriber = subscriber.with(tracing_journald::layer()?);
        return subscriber.init();
    }

    let layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let subscriber = subscriber.with(layer);
    subscriber.init();
}
