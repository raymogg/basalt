use crate::cache;
use crate::constants::{V4_QUOTER, V4_STATE_VIEW};
use crate::types::{QuoteResult, V4PoolInfo, V4PoolKey};
use alloy::primitives::{keccak256, Address, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;
use anyhow::Result;

// V4 StateView interface (for verifying cached pools)
sol! {
    #[sol(rpc)]
    interface IStateView {
        function getSlot0(bytes32 poolId) external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint24 protocolFee,
            uint24 lpFee
        );
        function getLiquidity(bytes32 poolId) external view returns (uint128 liquidity);
    }
}

// V4 Quoter interface
sol! {
    #[sol(rpc)]
    interface IV4Quoter {
        struct PoolKey {
            address currency0;
            address currency1;
            uint24 fee;
            int24 tickSpacing;
            address hooks;
        }

        struct QuoteParams {
            PoolKey poolKey;
            bool zeroForOne;
            uint128 exactAmount;
            bytes hookData;
        }

        function quoteExactInputSingle(QuoteParams calldata params) external returns (
            uint256 amountOut,
            uint256 gasEstimate
        );
    }
}

/// Compute V4 pool ID from pool key
fn compute_pool_id(pool_key: &V4PoolKey) -> [u8; 32] {
    let encoded = alloy::sol_types::SolValue::abi_encode(&(
        pool_key.currency0,
        pool_key.currency1,
        pool_key.fee,
        pool_key.tick_spacing,
        pool_key.hooks,
    ));
    keccak256(&encoded).into()
}

/// Try to find a pool from the persistent V4 pool registry and verify it on-chain.
/// Returns Some(pool_info) if a cached pool is still active.
async fn try_registry_pools(
    rpc_url: &str,
    token_address: Address,
    quote_token: Address,
) -> Result<Option<V4PoolInfo>> {
    let known_pools = cache::get_v4_pools_for_pair(token_address, quote_token)?;
    if known_pools.is_empty() {
        return Ok(None);
    }

    let zero_for_one = token_address < quote_token;

    eprintln!("[V4 DISCOVERY] Checking {} cached pool(s) from registry", known_pools.len());

    let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let state_view = IStateView::new(V4_STATE_VIEW.parse()?, provider);

    for pool_key in known_pools {
        let pool_id = compute_pool_id(&pool_key);
        match state_view.getSlot0(pool_id.into()).call().await {
            Ok(slot0) => {
                if slot0.sqrtPriceX96 > alloy::primitives::Uint::<160, 3>::ZERO {
                    let liquidity = state_view
                        .getLiquidity(pool_id.into())
                        .call()
                        .await
                        .map(|l| l.liquidity)
                        .unwrap_or(0);

                    eprintln!("[V4 DISCOVERY] Registry hit: fee={}, tickSpacing={}, hooks={}, liquidity={}",
                        pool_key.fee, pool_key.tick_spacing, pool_key.hooks, liquidity);

                    return Ok(Some(V4PoolInfo {
                        pool_key,
                        pool_id,
                        sqrt_price_x96: slot0.sqrtPriceX96,
                        liquidity,
                        zero_for_one,
                    }));
                }
            }
            Err(_) => {}
        }
    }

    Ok(None)
}

/// Discover V4 pool key for a token pair from local caches only.
/// Checks: 1) in-memory cache, 2) persistent registry (verified on-chain).
/// PoolScout data should be pre-fetched and cached by the caller (quote.rs).
pub async fn discover_v4_pool_key(
    rpc_url: &str,
    token_address: Address,
    quote_token: Address,
) -> Result<Option<V4PoolInfo>> {
    // 1. Check in-memory cache
    let cache_key = cache::pool_cache_key(token_address, quote_token);
    if let Some(cached) = cache::get_cached_pool(&cache_key) {
        return Ok(Some(cached));
    }

    // 2. Check persistent V4 pool registry
    match try_registry_pools(rpc_url, token_address, quote_token).await {
        Ok(Some(pool_info)) => {
            cache::cache_pool(cache_key, pool_info.clone());
            return Ok(Some(pool_info));
        }
        Ok(None) => {
            eprintln!("[V4 DISCOVERY] No match in registry for {} / {}",
                token_address, quote_token);
        }
        Err(e) => {
            eprintln!("[ERROR] Registry lookup failed: {}", e);
        }
    }

    Ok(None)
}

/// Get quote from V4 Quoter
pub async fn quote_v4(
    rpc_url: &str,
    _token_address: Address,
    amount: U256,
    pool_info: &V4PoolInfo,
) -> Result<QuoteResult> {
    let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let quoter = IV4Quoter::new(V4_QUOTER.parse()?, provider);

    let params = IV4Quoter::QuoteParams {
        poolKey: IV4Quoter::PoolKey {
            currency0: pool_info.pool_key.currency0,
            currency1: pool_info.pool_key.currency1,
            fee: pool_info.pool_key.fee_as_u24(),
            tickSpacing: pool_info.pool_key.tick_spacing_as_i24(),
            hooks: pool_info.pool_key.hooks,
        },
        zeroForOne: pool_info.zero_for_one,
        exactAmount: amount.to::<u128>(),
        hookData: alloy::primitives::Bytes::new(),
    };

    match quoter.quoteExactInputSingle(params).call().await {
        Ok(result) => {
            Ok(QuoteResult {
                method: format!(
                    "v4-direct({},{},{})",
                    pool_info.pool_key.fee, pool_info.pool_key.tick_spacing, pool_info.pool_key.hooks
                ),
                amount_out: result.amountOut,
                gas_estimate: Some(result.gasEstimate),
                pool_id: Some(format!("0x{}", alloy::primitives::hex::encode(pool_info.pool_id))),
            })
        }
        Err(e) => {
            eprintln!("[ERROR] V4 quote failed: {}", e);
            anyhow::bail!("V4 quote failed: {}", e);
        }
    }
}
