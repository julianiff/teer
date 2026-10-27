use clap::Parser;

#[tokio::main]
async fn main() {
    let args = teer::cli::Args::parse();
    if let Err(e) = teer::cli::run(args).await {
        eprintln!("teer: {e:#}");
        std::process::exit(1);
    }
}
