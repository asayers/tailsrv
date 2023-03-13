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
        subscriber.with(tracing_journald::layer()?).init()
    } else {
        subscriber.with(tracing_subscriber::fmt::layer()).init();
    }
    #[cfg(not(feature = "tracing-journald"))]
    subscriber.with(tracing_subscriber::fmt::layer()).init();
}
