use crate::cache;
use crate::constants::{CLANKER_HOOKS, POOL_MANAGER, V4_FEE_TICK_COMBOS, V4_QUOTER, V4_STATE_VIEW};
use crate::types::{QuoteResult, V4PoolInfo, V4PoolKey};
use alloy::primitives::{keccak256, Address, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Filter;
use alloy::sol;
use alloy::sol_types::SolEvent;
use anyhow::Result;

// V4 PoolManager interface
sol! {
    #[sol(rpc)]
    interface IPoolManager {
        event Initialize(
            bytes32 indexed id,
            address indexed currency0,
            address indexed currency1,
            uint24 fee,
            int24 tickSpacing,
            address hooks
        );
    }
}

// V4 StateView interface
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
    // PoolId = keccak256(abi.encode(poolKey))
    let encoded = alloy::sol_types::SolValue::abi_encode(&(
        pool_key.currency0,
        pool_key.currency1,
        pool_key.fee,
        pool_key.tick_spacing,
        pool_key.hooks,
    ));

    keccak256(&encoded).into()
}

/// Query the Initialize event to discover pool parameters for a specific pool ID
/// If creation_block_hint is provided, searches a narrow range around that block (~200 blocks)
/// Otherwise, searches the last 5M blocks (about 2.5 months on Base with 2-second blocks)
async fn discover_pool_params_from_events(
    rpc_url: &str,
    pool_id: [u8; 32],
    creation_block_hint: Option<u64>,
) -> Result<Option<V4PoolKey>> {
    let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);

    // Create filter for Initialize events with this pool ID
    let pool_manager_addr: Address = POOL_MANAGER.parse()?;
    let pool_id_fixed: FixedBytes<32> = FixedBytes::from(pool_id);

    let filter = Filter::new()
        .address(pool_manager_addr)
        .event_signature(IPoolManager::Initialize::SIGNATURE_HASH)
        .topic1(pool_id_fixed); // id is the first indexed parameter

    // Determine block range based on hint
    let latest_block = provider.get_block_number().await?;
    let from_block = if let Some(hint_block) = creation_block_hint {
        // Search narrow range: ±100 blocks around the hint (~3.3 minutes on Base)
        hint_block.saturating_sub(100)
    } else {
        // Fallback: Search last 5M blocks (about 2.5 months on Base with 2-second blocks)
        latest_block.saturating_sub(5_000_000)
    };

    let to_block = if let Some(hint_block) = creation_block_hint {
        hint_block.saturating_add(100).min(latest_block)
    } else {
        latest_block
    };

    let logs = provider
        .get_logs(&filter.from_block(from_block).to_block(to_block))
        .await?;

    if let Some(log) = logs.first() {
        // Decode the event
        let topics: Vec<_> = log.topics().iter().copied().collect();
        let event = IPoolManager::Initialize::decode_raw_log(&topics, &log.data().data, true)?;

        return Ok(Some(V4PoolKey {
            currency0: event.currency0,
            currency1: event.currency1,
            fee: event.fee.try_into()?,
            tick_spacing: event.tickSpacing.try_into()?,
            hooks: event.hooks,
        }));
    }

    Ok(None)
}

/// Discover V4 pool key by trying known Clanker configurations
/// If target_pool_id is provided, tries event querying first, then brute-force
/// If creation_block_hint is provided, uses a narrow block range for faster event queries
pub async fn discover_v4_pool_key_with_target(
    rpc_url: &str,
    token_address: Address,
    quote_token: Address,
    target_pool_id: Option<[u8; 32]>,
    creation_block_hint: Option<u64>,
) -> Result<Option<V4PoolInfo>> {
    // Check cache first
    let cache_key = cache::pool_cache_key(token_address, quote_token);
    if let Some(cached) = cache::get_cached_pool(&cache_key) {
        // If target_pool_id is specified, verify it matches
        if let Some(target) = target_pool_id {
            if cached.pool_id == target {
                return Ok(Some(cached));
            }
        } else {
            return Ok(Some(cached));
        }
    }

    // If we have a target pool ID, try querying the Initialize event first
    // (silently falls back to brute-force if RPC doesn't support log queries)
    if let Some(target) = target_pool_id {
        if let Ok(Some(pool_key)) = discover_pool_params_from_events(rpc_url, target, creation_block_hint).await {
            // Verify the pool exists and has liquidity
            let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
            let state_view = IStateView::new(V4_STATE_VIEW.parse()?, provider);

            if let Ok(slot0) = state_view.getSlot0(target.into()).call().await {
                if slot0.sqrtPriceX96 > alloy::primitives::Uint::<160, 3>::ZERO {
                    let liquidity = state_view
                        .getLiquidity(target.into())
                        .call()
                        .await
                        .map(|l| l.liquidity)
                        .unwrap_or(0);

                    let zero_for_one = token_address < quote_token;

                    let pool_info = V4PoolInfo {
                        pool_key,
                        pool_id: target,
                        sqrt_price_x96: slot0.sqrtPriceX96,
                        liquidity,
                        zero_for_one,
                    };

                    // Cache for future use
                    cache::cache_pool(cache_key.clone(), pool_info.clone());

                    return Ok(Some(pool_info));
                }
            }
        }
    }

    let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let state_view = IStateView::new(V4_STATE_VIEW.parse()?, provider);

    // Sort currencies (V4 requires currency0 < currency1)
    let zero_for_one = token_address < quote_token;
    let (currency0, currency1) = if zero_for_one {
        (token_address, quote_token)
    } else {
        (quote_token, token_address)
    };

    // Try each fee/tickSpacing/hooks combination
    // Note: Not parallelized as we want to return immediately upon first match
    // and most successful lookups hit early in the iteration
    for &(fee, tick_spacing) in V4_FEE_TICK_COMBOS {
        for &hooks_str in CLANKER_HOOKS {
            let hooks: Address = hooks_str.parse()?;

            let pool_key = V4PoolKey {
                currency0,
                currency1,
                fee,
                tick_spacing,
                hooks,
            };

            let pool_id = compute_pool_id(&pool_key);

            // If target_pool_id is specified, skip if this doesn't match
            if let Some(target) = target_pool_id {
                if pool_id != target {
                    continue;
                }
            }

            // Try to get slot0 - if it succeeds and sqrtPriceX96 > 0, pool exists
            match state_view.getSlot0(pool_id.into()).call().await {
                Ok(slot0) => {
                    if slot0.sqrtPriceX96 > alloy::primitives::Uint::<160, 3>::ZERO {
                        // Pool exists! Get liquidity
                        let liquidity = state_view
                            .getLiquidity(pool_id.into())
                            .call()
                            .await
                            .map(|l| l.liquidity)
                            .unwrap_or(0);

                        let pool_info = V4PoolInfo {
                            pool_key,
                            pool_id,
                            sqrt_price_x96: slot0.sqrtPriceX96,
                            liquidity,
                            zero_for_one,
                        };

                        // Cache for future use
                        cache::cache_pool(cache_key.clone(), pool_info.clone());

                        return Ok(Some(pool_info));
                    }
                }
                Err(_) => continue, // Pool doesn't exist with this config
            }
        }
    }

    Ok(None)
}

/// Discover V4 pool key by trying known Clanker configurations (backward compatible version)
pub async fn discover_v4_pool_key(
    rpc_url: &str,
    token_address: Address,
    quote_token: Address,
) -> Result<Option<V4PoolInfo>> {
    discover_v4_pool_key_with_target(rpc_url, token_address, quote_token, None, None).await
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
        Ok(result) => Ok(QuoteResult {
            // Store pool params in method for caching
            method: format!(
                "v4-direct({},{},{})",
                pool_info.pool_key.fee, pool_info.pool_key.tick_spacing, pool_info.pool_key.hooks
            ),
            amount_out: result.amountOut,
            gas_estimate: Some(result.gasEstimate),
        }),
        Err(e) => {
            // V4 Quoter might revert with the quote in the error data
            // Try to decode it
            anyhow::bail!("V4 quote failed: {}", e);
        }
    }
}
