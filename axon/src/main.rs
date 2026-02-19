use std::process::ExitCode;

use clap::Parser;

mod app;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = app::run::Cli::parse();
    app::run::init_tracing(cli.verbose, cli.quiet);
    match app::run::run(cli).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::from(1)
        }
    }
}
