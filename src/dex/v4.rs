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
            address hooks,
            uint160 sqrtPriceX96,
            int24 tick
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
    let block_range_size = 1800; // ±1800 blocks = ~1 hour on Base (2s/block)

    let from_block = if let Some(hint_block) = creation_block_hint {
        hint_block.saturating_sub(block_range_size)
    } else {
        // Fallback: Search last 5M blocks (about 2.5 months on Base with 2-second blocks)
        latest_block.saturating_sub(5_000_000)
    };

    let to_block = if let Some(hint_block) = creation_block_hint {
        hint_block.saturating_add(block_range_size).min(latest_block)
    } else {
        latest_block
    };

    // Calculate approximate time range (2 seconds per block on Base)
    let blocks_from_latest = latest_block.saturating_sub(from_block);
    let seconds_from_latest = blocks_from_latest * 2;
    let minutes_ago = seconds_from_latest / 60;

    eprintln!("[V4 DISCOVERY] Searching for Initialize event for pool {:?}",
        alloy::primitives::hex::encode(&pool_id));
    eprintln!("[V4 DISCOVERY]   PoolManager address: {}", POOL_MANAGER);
    eprintln!("[V4 DISCOVERY]   Block range: {} to {} (latest: {})",
        from_block, to_block, latest_block);
    eprintln!("[V4 DISCOVERY]   Time range: ~{} to ~{} minutes ago",
        minutes_ago, latest_block.saturating_sub(to_block) * 2 / 60);
    if let Some(hint) = creation_block_hint {
        eprintln!("[V4 DISCOVERY]   Hint block: {} (±{})", hint, block_range_size);
    }

    eprintln!("[V4 DISCOVERY]   Querying logs...");

    // Try with retry logic for rate limits
    let mut attempts = 0;
    let max_attempts = 3;
    let logs = loop {
        attempts += 1;
        // Clone the filter for each attempt since it gets consumed
        let query_filter = filter
            .clone()
            .from_block(from_block)
            .to_block(to_block);

        match provider.get_logs(&query_filter).await {
                Ok(logs) => {
                    break logs;
                }
                Err(e) => {
                    let err_str = e.to_string();

                    // Check if it's a rate limit error
                    if err_str.contains("429") || err_str.contains("Too Many Requests") || err_str.contains("rate limit") {
                        if attempts < max_attempts {
                            let delay_ms = 1000 * attempts; // 1s, 2s, 3s
                            eprintln!("[V4 DISCOVERY] Rate limited, retrying in {}ms (attempt {}/{})", delay_ms, attempts, max_attempts);
                            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;
                            continue;
                        } else {
                            eprintln!("[ERROR] V4 event query unavailable after {} attempts (rate limited)", max_attempts);
                            // Return Ok(None) to trigger brute-force fallback instead of erroring
                            return Ok(None);
                        }
                    } else {
                        // Non-rate-limit error, return it
                        eprintln!("[ERROR] V4 event query failed: {}", err_str);
                        return Err(e.into());
                    }
                }
            }
    };

    if let Some(log) = logs.first() {
        // Decode the event
        let topics: Vec<_> = log.topics().iter().copied().collect();

        match IPoolManager::Initialize::decode_raw_log(&topics, &log.data().data, true) {
            Ok(event) => {
                eprintln!("[V4 DISCOVERY] Found pool via Initialize event");
                eprintln!("[V4 DISCOVERY]   fee={}, tickSpacing={}, hooks={}",
                    event.fee, event.tickSpacing, event.hooks);

                return Ok(Some(V4PoolKey {
                    currency0: event.currency0,
                    currency1: event.currency1,
                    fee: event.fee.try_into()?,
                    tick_spacing: event.tickSpacing.try_into()?,
                    hooks: event.hooks,
                }));
            }
            Err(e) => {
                eprintln!("[ERROR] Failed to decode Initialize event: {}", e);
                return Err(e.into());
            }
        }
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
    if let Some(target) = target_pool_id {
        eprintln!("[V4 DISCOVERY] Attempting event-based discovery for pool 0x{}",
            alloy::primitives::hex::encode(&target));

        match discover_pool_params_from_events(rpc_url, target, creation_block_hint).await {
            Ok(Some(pool_key)) => {
                eprintln!("[V4 DISCOVERY] Event discovery succeeded, verifying pool on-chain");

                // Verify the pool exists and has liquidity
                let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
                let state_view = IStateView::new(V4_STATE_VIEW.parse()?, provider);

                match state_view.getSlot0(target.into()).call().await {
                    Ok(slot0) => {
                        if slot0.sqrtPriceX96 > alloy::primitives::Uint::<160, 3>::ZERO {
                            let liquidity = state_view
                                .getLiquidity(target.into())
                                .call()
                                .await
                                .map(|l| l.liquidity)
                                .unwrap_or(0);

                            let zero_for_one = token_address < quote_token;

                            eprintln!("[V4 DISCOVERY] Pool verified: liquidity={}", liquidity);

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
                        } else {
                            eprintln!("[ERROR] Pool has zero price, not initialized");
                        }
                    }
                    Err(e) => {
                        eprintln!("[ERROR] Failed to get slot0: {}", e);
                    }
                }
            }
            Ok(None) => {
                eprintln!("[V4 DISCOVERY] Event discovery returned None, falling back to brute-force");
            }
            Err(e) => {
                eprintln!("[ERROR] Event discovery error: {}, falling back to brute-force", e);
            }
        }
    }

    eprintln!("[V4 DISCOVERY] Starting brute-force discovery");

    // Sort currencies (V4 requires currency0 < currency1)
    let zero_for_one = token_address < quote_token;
    let (currency0, currency1) = if zero_for_one {
        (token_address, quote_token)
    } else {
        (quote_token, token_address)
    };

    // First check the V4 pool registry for known pools
    let mut pool_candidates = Vec::new();

    if let Ok(known_pools) = cache::get_v4_pools_for_pair(token_address, quote_token) {
        for pool_key in known_pools {
            let pool_id = compute_pool_id(&pool_key);

            // If target_pool_id is specified, skip if this doesn't match
            if let Some(target) = target_pool_id {
                if pool_id != target {
                    continue;
                }
            }

            pool_candidates.push((pool_key, pool_id));
        }
    }

    // If no known pools found in registry, build all possible combinations for brute-force
    if pool_candidates.is_empty() {
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

                pool_candidates.push((pool_key, pool_id));
            }
        }
    }

    eprintln!("[V4 DISCOVERY] Checking {} pool candidates", pool_candidates.len());

    // Check all candidates in parallel
    let mut tasks = Vec::new();
    for (pool_key, pool_id) in pool_candidates {
        let rpc_url = rpc_url.to_string();
        let task = tokio::spawn(async move {
            let provider = ProviderBuilder::new().on_http(rpc_url.parse().ok()?);
            let state_view = IStateView::new(V4_STATE_VIEW.parse().ok()?, provider);

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

                        return Some((pool_key, pool_id, slot0.sqrtPriceX96, liquidity));
                    }
                }
                Err(_) => {}
            }
            None
        });
        tasks.push(task);
    }

    // Wait for all tasks and return first successful match
    for task in tasks {
        if let Ok(Some((pool_key, pool_id, sqrt_price_x96, liquidity))) = task.await {
            eprintln!("[V4 DISCOVERY] Found active pool");
            eprintln!("[V4 DISCOVERY]   fee={}, tickSpacing={}, hooks={}",
                pool_key.fee, pool_key.tick_spacing, pool_key.hooks);
            eprintln!("[V4 DISCOVERY]   liquidity={}", liquidity);

            let pool_info = V4PoolInfo {
                pool_key,
                pool_id,
                sqrt_price_x96,
                liquidity,
                zero_for_one,
            };

            // Cache for future use
            cache::cache_pool(cache_key.clone(), pool_info.clone());

            return Ok(Some(pool_info));
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
        Ok(result) => {
            Ok(QuoteResult {
                // Store pool params in method for caching
                method: format!(
                    "v4-direct({},{},{})",
                    pool_info.pool_key.fee, pool_info.pool_key.tick_spacing, pool_info.pool_key.hooks
                ),
                amount_out: result.amountOut,
                gas_estimate: Some(result.gasEstimate),
            })
        }
        Err(e) => {
            eprintln!("[ERROR] V4 quote failed: {}", e);
            anyhow::bail!("V4 quote failed: {}", e);
        }
    }
}
