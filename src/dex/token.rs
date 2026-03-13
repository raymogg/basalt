use crate::cache;
use crate::types::{TokenBalance, TokenMetadata};
use alloy::primitives::{Address, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;

use anyhow::Result;

// ERC20 interface
sol! {
    #[sol(rpc)]
    interface IERC20 {
        function balanceOf(address account) external view returns (uint256);
        function decimals() external view returns (uint8);
        function symbol() external view returns (string memory);
        function name() external view returns (string memory);
    }
}

/// Fetch token metadata from on-chain, using cache if available
pub async fn get_token_metadata(rpc_url: &str, token_address: Address) -> Result<TokenMetadata> {
    // Try cache first
    if let Some(cached) = cache::get_cached_token(token_address)? {
        return Ok(TokenMetadata {
            address: token_address,
            decimals: cached.decimals,
            symbol: cached.symbol,
            name: cached.name,
        });
    }

    // Fetch from on-chain
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let token = IERC20::new(token_address, provider);

    let decimals = token.decimals().call().await?;
    let symbol = token.symbol().call().await?;
    let name = token.name().call().await?;

    let metadata = TokenMetadata {
        address: token_address,
        decimals,
        symbol,
        name,
    };

    // Cache the metadata
    cache::cache_token(&metadata)?;

    Ok(metadata)
}

/// Get token balance for a specific wallet
pub async fn get_token_balance(
    rpc_url: &str,
    token_address: Address,
    wallet: Address,
) -> Result<TokenBalance> {
    let metadata = get_token_metadata(rpc_url, token_address).await?;

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let token = IERC20::new(token_address, provider);

    let balance_result = token.balanceOf(wallet).call().await?;

    Ok(TokenBalance {
        metadata,
        balance: balance_result,
    })
}

/// Format token amount with decimals
pub fn format_token_amount(amount: U256, decimals: u8) -> f64 {
    let divisor = 10_f64.powi(decimals as i32);
    amount.to_string().parse::<f64>().unwrap_or(0.0) / divisor
}

/// Parse token amount from string with decimals
pub fn parse_token_amount(amount_str: &str, decimals: u8) -> Result<U256> {
    let amount: f64 = amount_str.parse()?;
    let multiplier = 10_f64.powi(decimals as i32);
    let raw = (amount * multiplier) as u128;
    Ok(U256::from(raw))
}
