use alloy::primitives::Address;
use std::collections::HashMap;

/// Get Base RPC URL from environment variable, or use default public RPCs
pub fn get_base_rpc() -> String {
    std::env::var("BASE_RPC_URL").unwrap_or_else(|_| {
        // Fallback to public RPCs (note: these may not support event log queries)
        "https://base.rpc.blxrbdn.com".to_string()
    })
}

// Legacy constant for backward compatibility (deprecated - use get_base_rpc())
pub const BASE_RPCS: &[&str] = &[
    "https://base.rpc.blxrbdn.com",
    "https://base-mainnet.infura.io/v3/6b2fff1be00247b4b84ebf8e7957f8f9",
];

// Token addresses on Base
pub const USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
pub const WETH: &str = "0x4200000000000000000000000000000000000006";
pub const FLETH: &str = "0x000000000D564D5be76f7f0d28fE52605afC7Cf8";
pub const ETH_ZERO: &str = "0x0000000000000000000000000000000000000000"; // native ETH in V4

// Contract addresses on Base
pub const POOL_MANAGER: &str = "0x498581ff718922c3f8e6a244956af099b2652b2b";
pub const V4_QUOTER: &str = "0x0d5e0f971ed27fbff6c2837bf31316121532048d";
pub const V4_STATE_VIEW: &str = "0xa3c0c9b65bad0b08107aa264b0f3db444b867a71";
pub const V3_QUOTER_V2: &str = "0x3d4e44Eb1374240CE5F1B871ab261CD16335B76a";
pub const AERO_ROUTER: &str = "0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43";
pub const AERO_DEFAULT_FACTORY: &str = "0x420DD381b31aEf6683db6B902084cB0FFECe40Da";

// Known Clanker V4 hooks addresses (discovered via Initialize events)
pub const CLANKER_HOOKS: &[&str] = &[
    "0x0000000000000000000000000000000000000000", // No hooks (vanilla V4)
    "0xbb7784a4d481184283ed89619a3e3ed143e1adc0", // hooks v1 (older pools)
    "0xd60d6b218116cfd801e28f78d011a203d2b068cc", // hooks v2
    "0xb429d62f8f3bffb98cdb9569533ea23bf0ba28cc", // hooks v3
    "0x23321f11a6d44fd1ab790044fdfde5758c902fdc", // hooks v4
    "0xF785bb58059FAB6fb19bDdA2CB9078d9E546Efdc", // hooks v5 (OSO/flETH and newer),
    "0x34a45c6B61876d739400Bd71228CbcbD4F53E8cC",
    "0xDd5EeaFf7BD481AD55Db083062b13a3cdf0A68CC",
];

// Known quote tokens for V4 pools
pub const V4_QUOTE_TOKENS: &[&str] = &[
    WETH,
    "0x000000000D564D5be76f7f0d28fE52605afC7Cf8", // flETH
];

// Fee/tickSpacing combinations for V4 pools
pub const V4_FEE_TICK_COMBOS: &[(u32, i32)] = &[
    (8388608, 200), // Clanker standard (dynamic fee = 0x800000)
    (8388608, 60),  // Dynamic fee, tickSpacing 60
    (0, 200),       // Dynamic fee flag, tickSpacing 200
    (0, 60),        // Dynamic fee, tickSpacing 60
    (10000, 200),   // 1% fee fallback
    (10000, 60),    // 1% fee, tickSpacing 60
    (3000, 60),     // 0.3% fee
    (3000, 200),    // 0.3% fee, tickSpacing 200
    (500, 10),      // 0.05% fee
    (100, 1),       // 0.01% fee
];

// V3 fee tiers
pub const V3_FEES: &[u32] = &[3000, 10000, 500];

/// Resolve token symbol to address (e.g., "usdc" -> USDC address)
pub fn resolve_token_address(input: &str) -> Option<Address> {
    // Try parsing as address first
    if let Ok(addr) = input.parse::<Address>() {
        return Some(addr);
    }

    // Check common aliases (case-insensitive)
    let aliases: HashMap<&str, &str> = [("usdc", USDC), ("weth", WETH), ("eth", WETH)]
        .iter()
        .cloned()
        .collect();

    aliases
        .get(input.to_lowercase().as_str())
        .and_then(|addr_str| addr_str.parse().ok())
}
