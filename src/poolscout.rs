use crate::types::{V4PoolInfo, V4PoolKey};
use alloy::primitives::Address;
use anyhow::Result;
use serde::Deserialize;

const POOLSCOUT_API_BASE: &str = "https://api.poolscout.dev";

/// Get the PoolScout API key from environment variable
fn get_api_key() -> Result<String> {
    std::env::var("POOLSCOUT_API_KEY")
        .map_err(|_| anyhow::anyhow!("POOLSCOUT_API_KEY environment variable not set. Sign up at poolscout.dev"))
}

#[derive(Debug, Deserialize, Clone)]
pub struct PoolScoutPool {
    pub pool_id: String,
    pub currency0: String,
    pub currency1: String,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: String,
    pub liquidity: String,
    pub sqrt_price_x96: String,
    pub tick: i32,
}

#[derive(Debug, Deserialize)]
struct PoolsByTokenResponse {
    pools: Vec<PoolScoutPool>,
    count: usize,
}

#[derive(Debug, Deserialize)]
struct PoolByIdResponse {
    pool: PoolScoutPool,
}

impl PoolScoutPool {
    /// Convert to V4PoolKey
    pub fn to_pool_key(&self) -> Result<V4PoolKey> {
        Ok(V4PoolKey {
            currency0: self.currency0.parse()?,
            currency1: self.currency1.parse()?,
            fee: self.fee,
            tick_spacing: self.tick_spacing,
            hooks: self.hooks.parse()?,
        })
    }

    /// Convert to V4PoolInfo with direction info
    pub fn to_pool_info(&self, token_address: Address, quote_token: Address) -> Result<V4PoolInfo> {
        let pool_key = self.to_pool_key()?;
        let zero_for_one = token_address < quote_token;

        // Parse pool_id hex string to [u8; 32]
        let pool_id_hex = self.pool_id.strip_prefix("0x").unwrap_or(&self.pool_id);
        let pool_id_bytes = alloy::hex::decode(pool_id_hex)?;
        let mut pool_id = [0u8; 32];
        pool_id.copy_from_slice(&pool_id_bytes);

        // Parse sqrt_price_x96
        let sqrt_price_x96: alloy::primitives::Uint<160, 3> = self.sqrt_price_x96.parse()
            .unwrap_or(alloy::primitives::Uint::<160, 3>::ZERO);

        // Parse liquidity
        let liquidity: u128 = self.liquidity.parse().unwrap_or(0);

        Ok(V4PoolInfo {
            pool_key,
            pool_id,
            sqrt_price_x96,
            liquidity,
            zero_for_one,
        })
    }

    /// Check if this pool has liquidity and is initialized
    pub fn is_active(&self) -> bool {
        self.sqrt_price_x96 != "0" && self.liquidity != "0"
    }

    /// Get the "other" token in the pair given one token
    pub fn other_token(&self, token: Address) -> Option<Address> {
        let c0: Address = self.currency0.parse().ok()?;
        let c1: Address = self.currency1.parse().ok()?;
        if c0 == token {
            Some(c1)
        } else if c1 == token {
            Some(c0)
        } else {
            None
        }
    }
}

/// Build an HTTP client with the API key header
fn build_client() -> Result<reqwest::Client> {
    let api_key = get_api_key()?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("X-API-Key", api_key.parse()?);
    Ok(reqwest::Client::builder()
        .default_headers(headers)
        .build()?)
}

/// Fetch all pools containing a specific token.
/// This is the single PoolScout call we make per quote — all routing
/// decisions are derived from this list.
pub async fn get_pools_by_token(token_address: Address) -> Result<Vec<PoolScoutPool>> {
    let client = build_client()?;
    let url = format!("{}/pools/by-token/{}", POOLSCOUT_API_BASE, token_address);

    eprintln!("[POOLSCOUT] Fetching pools for token {}", token_address);

    let response = client
        .get(&url)
        .query(&[("include_zero_liquidity", "false"), ("limit", "100")])
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("PoolScout API error ({}): {}", status, body);
    }

    let data: PoolsByTokenResponse = response.json().await?;
    eprintln!("[POOLSCOUT] Found {} active pools for token {}", data.count, token_address);

    Ok(data.pools)
}

/// Find the best pool for a specific token pair from a pre-fetched pool list.
/// No API calls — purely filters the provided list.
pub fn find_pool_for_pair_from_list(
    pools: &[PoolScoutPool],
    token_a: Address,
    token_b: Address,
) -> Option<PoolScoutPool> {
    pools
        .iter()
        .filter(|p| {
            p.is_active() && p.other_token(token_a).map_or(false, |other| other == token_b)
        })
        .max_by_key(|p| p.liquidity.parse::<u128>().unwrap_or(0))
        .cloned()
}

/// Extract routing info from a pre-fetched pool list.
/// Groups pools by the other token, keeping the best pool per pair.
/// No API calls — purely processes the provided list.
pub fn get_routing_pools_from_list(
    pools: &[PoolScoutPool],
    token_address: Address,
) -> Vec<(Address, PoolScoutPool)> {
    let mut best_by_quote: std::collections::HashMap<Address, PoolScoutPool> =
        std::collections::HashMap::new();

    for pool in pools {
        if !pool.is_active() {
            continue;
        }

        if let Some(other) = pool.other_token(token_address) {
            let liq: u128 = pool.liquidity.parse().unwrap_or(0);
            let should_insert = match best_by_quote.get(&other) {
                Some(existing) => {
                    let existing_liq: u128 = existing.liquidity.parse().unwrap_or(0);
                    liq > existing_liq
                }
                None => true,
            };

            if should_insert {
                best_by_quote.insert(other, pool.clone());
            }
        }
    }

    let mut result: Vec<_> = best_by_quote.into_iter().collect();
    result.sort_by(|a, b| {
        let liq_a: u128 = a.1.liquidity.parse().unwrap_or(0);
        let liq_b: u128 = b.1.liquidity.parse().unwrap_or(0);
        liq_b.cmp(&liq_a)
    });

    result
}
