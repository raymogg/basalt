use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMetadata {
    pub address: Address,
    pub decimals: u8,
    pub symbol: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct TokenBalance {
    pub metadata: TokenMetadata,
    pub balance: U256,
}

#[derive(Debug, Clone)]
pub struct V4PoolKey {
    pub currency0: Address,
    pub currency1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: Address,
}

impl V4PoolKey {
    pub fn fee_as_u24(&self) -> alloy::primitives::Uint<24, 1> {
        alloy::primitives::Uint::<24, 1>::from(self.fee)
    }

    pub fn tick_spacing_as_i24(&self) -> alloy::primitives::Signed<24, 1> {
        alloy::primitives::Signed::<24, 1>::try_from(self.tick_spacing).unwrap()
    }
}

#[derive(Debug, Clone)]
pub struct V4PoolInfo {
    pub pool_key: V4PoolKey,
    pub pool_id: [u8; 32],
    pub sqrt_price_x96: alloy::primitives::Uint<160, 3>,
    pub liquidity: u128,
    pub zero_for_one: bool,
}

#[derive(Debug, Clone)]
pub struct QuoteResult {
    pub method: String,
    pub amount_out: U256,
    pub gas_estimate: Option<U256>,
}

#[derive(Debug, Clone)]
pub struct FullQuoteResult {
    pub token: TokenBalance,
    pub quotes: Vec<QuoteResult>,
    pub best_quote: Option<QuoteResult>,
}

// Cache structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRoute {
    pub method: String,
    pub relative_performance: f64, // % difference from best route (0.0 = best, 0.5 = 0.5% worse)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCache {
    pub routes: Vec<CachedRoute>, // Top routes within 0.5% of best (max 3)
    pub updated_at: u64,
    pub usage_count: u64, // Number of times this cache has been used
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRegistry {
    pub decimals: u8,
    pub symbol: String,
    pub name: String,
    pub cached_at: u64,
}

// V4 Pool Registry - stores all known valid V4 pools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V4PoolRegistry {
    // Key: "token0_token1" (sorted), Value: pool parameters
    pub pools: std::collections::HashMap<String, V4PoolKeyData>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V4PoolKeyData {
    pub currency0: String, // Address as string
    pub currency1: String, // Address as string
    pub fee: u32,
    pub tick_spacing: i32,
    pub hooks: String, // Address as string
    pub discovered_at: u64,
}
