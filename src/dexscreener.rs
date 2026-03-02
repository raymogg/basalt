use alloy::primitives::Address;
use alloy::hex;
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct DexScreenerPair {
    #[serde(rename = "chainId")]
    pub chain_id: String,
    #[serde(rename = "dexId")]
    pub dex_id: String,
    #[serde(rename = "pairAddress")]
    pub pair_address: String,
    #[serde(rename = "baseToken")]
    pub base_token: TokenInfo,
    #[serde(rename = "quoteToken")]
    pub quote_token: TokenInfo,
    #[serde(rename = "priceUsd")]
    pub price_usd: Option<String>,
    pub liquidity: Option<Liquidity>,
    pub labels: Option<Vec<String>>,
    #[serde(rename = "pairCreatedAt")]
    pub pair_created_at: Option<u64>, // Unix timestamp in milliseconds
}

#[derive(Debug, Deserialize, Clone)]
pub struct TokenInfo {
    pub address: String,
    pub symbol: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Liquidity {
    pub usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DexScreenerResponse {
    pairs: Option<Vec<DexScreenerPair>>,
}

impl DexScreenerPair {
    /// Get the DEX version (v2, v3, v4) from labels
    pub fn version(&self) -> Option<&str> {
        if let Some(labels) = &self.labels {
            if labels.contains(&"v4".to_string()) {
                return Some("v4");
            }
            if labels.contains(&"v3".to_string()) {
                return Some("v3");
            }
            if labels.contains(&"v2".to_string()) {
                return Some("v2");
            }
        }
        None
    }

    /// Get liquidity in USD
    pub fn liquidity_usd(&self) -> f64 {
        self.liquidity
            .as_ref()
            .and_then(|l| l.usd)
            .unwrap_or(0.0)
    }

    /// Get pool ID (for V4 pools, pair_address is actually the pool ID)
    pub fn pool_id(&self) -> Option<[u8; 32]> {
        // Pool ID is 32 bytes (64 hex chars + 0x prefix = 66 chars)
        if self.pair_address.len() == 66 && self.pair_address.starts_with("0x") {
            let hex_str = &self.pair_address[2..];
            let mut bytes = [0u8; 32];
            match hex::decode_to_slice(hex_str, &mut bytes) {
                Ok(_) => Some(bytes),
                Err(e) => {
                    eprintln!("[ERROR] Failed to decode pool ID hex: {}", e);
                    None
                }
            }
        } else {
            None
        }
    }

    /// Parse quote token address
    pub fn quote_token_address(&self) -> Option<Address> {
        self.quote_token.address.parse().ok()
    }

    /// Estimate block number from creation timestamp (Base mainnet)
    /// Base averages ~2 seconds per block
    pub fn estimate_creation_block(&self, current_block: u64, current_timestamp: u64) -> Option<u64> {
        let creation_timestamp_secs = self.pair_created_at? / 1000; // Convert ms to seconds

        if creation_timestamp_secs > current_timestamp {
            return None; // Future timestamp, invalid
        }

        const BASE_SECONDS_PER_BLOCK: u64 = 2;
        let seconds_ago = current_timestamp.saturating_sub(creation_timestamp_secs);
        let blocks_ago = seconds_ago / BASE_SECONDS_PER_BLOCK;

        Some(current_block.saturating_sub(blocks_ago))
    }
}

/// Fetch pool data from DexScreener API for a given token address
pub async fn get_pairs(token_address: Address) -> Result<Vec<DexScreenerPair>> {
    let url = format!(
        "https://api.dexscreener.com/latest/dex/tokens/{}",
        token_address
    );

    let response = reqwest::get(&url).await?;
    let data: DexScreenerResponse = response.json().await?;

    let pairs = data.pairs.unwrap_or_default();

    // Filter to Base chain only and sort by liquidity
    let mut base_pairs: Vec<_> = pairs
        .into_iter()
        .filter(|p| p.chain_id.to_lowercase() == "base")
        .collect();

    base_pairs.sort_by(|a, b| {
        b.liquidity_usd()
            .partial_cmp(&a.liquidity_usd())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(base_pairs)
}

/// Get V4 pairs for a token (sorted by liquidity, highest first)
pub async fn get_v4_pairs(token_address: Address) -> Result<Vec<DexScreenerPair>> {
    let pairs = get_pairs(token_address).await?;
    Ok(pairs
        .into_iter()
        .filter(|p| p.version() == Some("v4"))
        .collect())
}
