mod cache;
mod constants;
mod dex;
mod dexscreener;
mod poolscout;
mod quote;
mod types;
mod universal_router;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "basalt")]
#[command(about = "CLI toolkit for AI agents trading on Base", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Get swap quote for a token pair
    Quote {
        /// Amount of input token to swap
        amount: String,

        /// Input token address (e.g. USDC address for buying)
        #[arg(short, long)]
        from: String,

        /// Output token address (e.g. token address for buying)
        #[arg(short, long)]
        to: String,

        /// Quote all portfolio positions
        #[arg(short, long)]
        portfolio: bool,
    },

    /// Refresh all cached routes by re-quoting each token pair
    RefreshCache {
        /// Amount to use for test quotes (default: 1.0 of input token)
        #[arg(long, default_value = "1.0")]
        test_amount: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for connection pool debugging
    // Set RUST_LOG=hyper=debug,reqwest=debug to see connection pool activity
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"))
        )
        .init();

    // Load .env file if it exists (silently ignore if not found)
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    match cli.command {
        Commands::Quote {
            amount,
            from,
            to,
            portfolio,
        } => {
            if portfolio {
                quote::portfolio_quotes().await?;
            } else {
                quote::quote_swap(&amount, &from, &to).await?;
            }
        }
        Commands::RefreshCache { test_amount } => {
            quote::refresh_all_cached_routes(&test_amount).await?;
        }
    }

    Ok(())
}
