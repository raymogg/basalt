use crate::cache;
use crate::constants::{get_base_rpc, V3_FEES, WETH, FLETH};
use crate::dex;
use crate::dexscreener;
use crate::types::{FullQuoteResult, QuoteResult};
use alloy::primitives::{Address, U256};
use anyhow::Result;

/// Get cache refresh interval from env var or use default
fn cache_refresh_interval() -> u64 {
    std::env::var("BASALT_CACHE_REFRESH_INTERVAL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10) // Default: refresh every 10 quotes
}

/// Get comprehensive quote for any token pair, trying all routes
pub async fn get_quote(
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    decimals_in: u8,
) -> Result<FullQuoteResult> {
    let rpc_url = get_base_rpc();

    // Get token metadata
    let token_in_meta = dex::get_token_metadata(&rpc_url, token_in).await?;
    let token_out_meta = dex::get_token_metadata(&rpc_url, token_out).await?;

    if amount_in == U256::ZERO {
        return Ok(FullQuoteResult {
            token: crate::types::TokenBalance {
                metadata: token_in_meta,
                balance: U256::ZERO,
            },
            quotes: vec![],
            best_quote: None,
        });
    }

    // Try cached routes first (up to 3 best routes within 0.5% of each other)
    let mut quotes = Vec::new();
    let refresh_interval = cache_refresh_interval();
    let cached_routes = cache::get_cached_routes(token_in, token_out, refresh_interval)
        .ok()
        .flatten();

    let needs_full_refresh = cached_routes.is_none();

    if let Some(route_cache) = cached_routes {
        // Try all cached routes in parallel
        for cached_route in &route_cache.routes {
            if let Ok(Some(quote)) = try_route(
                &rpc_url,
                token_in,
                token_out,
                amount_in,
                &cached_route.method,
            )
            .await
            {
                quotes.push(quote);
            }
        }
    }

    // If cache miss or periodic refresh, do full fan-out
    if needs_full_refresh {
        quotes = try_all_routes(&rpc_url, token_in, token_out, amount_in).await?;

        // Cache the top routes (within 0.5% of best, max 3)
        let _ = cache::cache_routes(token_in, token_out, &quotes);
    }

    // Find best quote
    let best_quote = quotes.iter().max_by_key(|q| q.amount_out).cloned();

    Ok(FullQuoteResult {
        token: crate::types::TokenBalance {
            metadata: token_in_meta,
            balance: amount_in,
        },
        quotes,
        best_quote,
    })
}

/// Try a specific route method (from cache - uses exact pool/fee specified)
async fn try_route(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount: U256,
    method: &str,
) -> Result<Option<QuoteResult>> {
    let weth_addr: Address = WETH.parse()?;

    // V3 direct with specific fee tier
    // Format: "v3-direct(500)" or "v3-direct(3000)" or "v3-direct(10000)"
    if method.starts_with("v3-direct(") {
        if let Some(fee_str) = method
            .strip_prefix("v3-direct(")
            .and_then(|s| s.strip_suffix(")"))
        {
            if let Ok(fee) = fee_str.parse::<u32>() {
                if let Ok(quote) = dex::quote_v3(rpc_url, token_in, token_out, amount, fee).await {
                    return Ok(Some(quote));
                }
            }
        }
    }

    // Aerodrome with specific pool type
    // Format: "aerodrome-stable" or "aerodrome-volatile"
    if method.starts_with("aerodrome-") {
        let stable = method == "aerodrome-stable";
        if let Ok(quote) = dex::quote_aerodrome(rpc_url, token_in, token_out, amount, stable).await
        {
            return Ok(Some(quote));
        }
    }

    // V4 direct with cached pool params
    // Format: "v4-direct(fee,tickSpacing,hooks)" e.g. "v4-direct(8388608,200,0xbb7784...)"
    if method.starts_with("v4-direct(") {
        if let Some(params_str) = method
            .strip_prefix("v4-direct(")
            .and_then(|s| s.strip_suffix(")"))
        {
            let parts: Vec<&str> = params_str.split(',').collect();
            if parts.len() == 3 {
                if let (Ok(fee), Ok(tick_spacing), Ok(hooks)) = (
                    parts[0].parse::<u32>(),
                    parts[1].parse::<i32>(),
                    parts[2].parse::<Address>(),
                ) {
                    // Reconstruct pool info from cached params
                    let zero_for_one = token_in < token_out;
                    let (currency0, currency1) = if zero_for_one {
                        (token_in, token_out)
                    } else {
                        (token_out, token_in)
                    };

                    let pool_key = crate::types::V4PoolKey {
                        currency0,
                        currency1,
                        fee,
                        tick_spacing,
                        hooks,
                    };

                    let pool_info = crate::types::V4PoolInfo {
                        pool_key,
                        pool_id: [0u8; 32], // Not needed for quoting
                        sqrt_price_x96: alloy::primitives::Uint::<160, 3>::ZERO, // Not needed
                        liquidity: 0,       // Not needed
                        zero_for_one,
                    };

                    if let Ok(quote) = dex::quote_v4(rpc_url, token_in, amount, &pool_info).await {
                        return Ok(Some(quote));
                    }
                }
            }
        }
    }

    // V4 direct fallback (old format without params - do full discovery)
    if method == "v4-direct" {
        if let Some(pool_info) = dex::discover_v4_pool_key(rpc_url, token_in, token_out).await? {
            if let Ok(quote) = dex::quote_v4(rpc_url, token_in, amount, &pool_info).await {
                return Ok(Some(quote));
            }
        }
    }

    // V4 multi-hop V4-V4 with cached params
    // Format: "v4-{token}-v4(v4_fee1,v4_tick1,v4_hooks1,v4_fee2,v4_tick2,v4_hooks2)"
    if method.starts_with("v4-") && method.contains("-v4(") {
        if let Some(rest) = method.strip_prefix("v4-") {
            let parts: Vec<&str> = rest.split("-v4(").collect();
            if parts.len() == 2 {
                let intermediate_symbol = parts[0];
                let params_str = parts[1].strip_suffix(")").unwrap_or("");
                let params: Vec<&str> = params_str.split(',').collect();

                if params.len() == 6 {
                    if let (Ok(v4_fee1), Ok(v4_tick1), Ok(v4_hooks1), Ok(v4_fee2), Ok(v4_tick2), Ok(v4_hooks2)) = (
                        params[0].parse::<u32>(),
                        params[1].parse::<i32>(),
                        params[2].parse::<Address>(),
                        params[3].parse::<u32>(),
                        params[4].parse::<i32>(),
                        params[5].parse::<Address>(),
                    ) {
                        let intermediate_addr = match intermediate_symbol {
                            "weth" => WETH.parse()?,
                            "fleth" => FLETH.parse()?,
                            _ => return Ok(None),
                        };

                        // Build first V4 pool (token_in -> intermediate)
                        let zero_for_one1 = token_in < intermediate_addr;
                        let (currency0_1, currency1_1) = if zero_for_one1 {
                            (token_in, intermediate_addr)
                        } else {
                            (intermediate_addr, token_in)
                        };

                        let pool_info1 = crate::types::V4PoolInfo {
                            pool_key: crate::types::V4PoolKey {
                                currency0: currency0_1,
                                currency1: currency1_1,
                                fee: v4_fee1,
                                tick_spacing: v4_tick1,
                                hooks: v4_hooks1,
                            },
                            pool_id: [0u8; 32],
                            sqrt_price_x96: alloy::primitives::Uint::<160, 3>::ZERO,
                            liquidity: 0,
                            zero_for_one: zero_for_one1,
                        };

                        // Execute first hop
                        if let Ok(v4_quote1) = dex::quote_v4(rpc_url, token_in, amount, &pool_info1).await {
                            // Build second V4 pool (intermediate -> token_out)
                            let zero_for_one2 = intermediate_addr < token_out;
                            let (currency0_2, currency1_2) = if zero_for_one2 {
                                (intermediate_addr, token_out)
                            } else {
                                (token_out, intermediate_addr)
                            };

                            let pool_info2 = crate::types::V4PoolInfo {
                                pool_key: crate::types::V4PoolKey {
                                    currency0: currency0_2,
                                    currency1: currency1_2,
                                    fee: v4_fee2,
                                    tick_spacing: v4_tick2,
                                    hooks: v4_hooks2,
                                },
                                pool_id: [0u8; 32],
                                sqrt_price_x96: alloy::primitives::Uint::<160, 3>::ZERO,
                                liquidity: 0,
                                zero_for_one: zero_for_one2,
                            };

                            // Execute second hop
                            if let Ok(v4_quote2) = dex::quote_v4(rpc_url, intermediate_addr, v4_quote1.amount_out, &pool_info2).await {
                                return Ok(Some(QuoteResult {
                                    method: method.to_string(),
                                    amount_out: v4_quote2.amount_out,
                                    gas_estimate: None,
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    // V4 multi-hop via intermediate token with cached params
    // Format: "v4-{token}-v3(v4_fee,v4_tick,v4_hooks,v3_fee)"
    // Example: "v4-fleth-v3(0,60,0xF785bb...,3000)"
    if method.starts_with("v4-") && method.contains("-v3(") {
        // Extract intermediate token symbol and params
        if let Some(rest) = method.strip_prefix("v4-") {
            let parts: Vec<&str> = rest.split("-v3(").collect();
            if parts.len() == 2 {
                let intermediate_symbol = parts[0];
                let params_str = parts[1].strip_suffix(")").unwrap_or("");
                let params: Vec<&str> = params_str.split(',').collect();

                if params.len() == 4 {
                    if let (Ok(v4_fee), Ok(v4_tick), Ok(v4_hooks), Ok(v3_fee)) = (
                        params[0].parse::<u32>(),
                        params[1].parse::<i32>(),
                        params[2].parse::<Address>(),
                        params[3].parse::<u32>(),
                    ) {
                        // Determine intermediate token address
                        let intermediate_addr = match intermediate_symbol {
                            "weth" => WETH.parse()?,
                            "fleth" => FLETH.parse()?,
                            _ => return Ok(None), // Unknown intermediate
                        };

                        // Build V4 pool key for token_in -> intermediate
                        let zero_for_one = token_in < intermediate_addr;
                        let (currency0, currency1) = if zero_for_one {
                            (token_in, intermediate_addr)
                        } else {
                            (intermediate_addr, token_in)
                        };

                        let v4_pool_key = crate::types::V4PoolKey {
                            currency0,
                            currency1,
                            fee: v4_fee,
                            tick_spacing: v4_tick,
                            hooks: v4_hooks,
                        };

                        let v4_pool_info = crate::types::V4PoolInfo {
                            pool_key: v4_pool_key,
                            pool_id: [0u8; 32],
                            sqrt_price_x96: alloy::primitives::Uint::<160, 3>::ZERO,
                            liquidity: 0,
                            zero_for_one,
                        };

                        // Execute: token_in -> intermediate via V4
                        if let Ok(v4_quote) = dex::quote_v4(rpc_url, token_in, amount, &v4_pool_info).await {
                            // Execute: intermediate -> token_out via V3
                            if let Ok(v3_quote) = dex::quote_v3(
                                rpc_url,
                                intermediate_addr,
                                token_out,
                                v4_quote.amount_out,
                                v3_fee,
                            ).await {
                                return Ok(Some(QuoteResult {
                                    method: method.to_string(),
                                    amount_out: v3_quote.amount_out,
                                    gas_estimate: None,
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    // V4 via WETH (multi-hop, old format - still needs discovery)
    if method.starts_with("v4-weth-v3") || method.starts_with("v4-via-weth") {
        if let Some(pool_info) = dex::discover_v4_pool_key(rpc_url, token_in, weth_addr).await? {
            if let Ok(v4_quote) = dex::quote_v4(rpc_url, token_in, amount, &pool_info).await {
                // For the second leg, try all V3 fees (TODO: could cache this too)
                if let Ok(Some(out_quote)) = dex::quote_v3_multi_fee(
                    rpc_url,
                    weth_addr,
                    token_out,
                    v4_quote.amount_out,
                    V3_FEES,
                )
                .await
                {
                    return Ok(Some(QuoteResult {
                        method: "v4-weth-v3".to_string(),
                        amount_out: out_quote.amount_out,
                        gas_estimate: None,
                    }));
                }
            }
        }
    }

    // V3 via WETH (multi-hop, try all fees for both legs)
    if method.starts_with("v3-via-weth") {
        if let Ok(Some(quote)) = try_v3_via_weth(rpc_url, token_in, token_out, amount).await {
            return Ok(Some(quote));
        }
    }

    Ok(None)
}

/// Get best V3 quote, using cache if available
/// This is cache-aware unlike quote_v3_multi_fee which always tries all fees
async fn get_best_v3_quote_cached(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount: U256,
) -> Result<Option<QuoteResult>> {
    // Check cache for this pair
    let refresh_interval = cache_refresh_interval();
    let cached_routes = cache::get_cached_routes(token_in, token_out, refresh_interval)
        .ok()
        .flatten();

    // If we have cached routes, try only the cached V3 fees
    if let Some(route_cache) = cached_routes {
        let mut best_quote: Option<QuoteResult> = None;

        for cached_route in &route_cache.routes {
            // Only process V3 direct routes
            if cached_route.method.starts_with("v3-direct(") {
                if let Some(fee_str) = cached_route.method
                    .strip_prefix("v3-direct(")
                    .and_then(|s| s.strip_suffix(")"))
                {
                    if let Ok(fee) = fee_str.parse::<u32>() {
                        if let Ok(quote) = dex::quote_v3(rpc_url, token_in, token_out, amount, fee).await {
                            if let Some(ref current_best) = best_quote {
                                if quote.amount_out > current_best.amount_out {
                                    best_quote = Some(quote);
                                }
                            } else {
                                best_quote = Some(quote);
                            }
                        }
                    }
                }
            }
        }

        if best_quote.is_some() {
            return Ok(best_quote);
        }
    }

    // Cache miss or no V3 routes in cache - try all fee tiers
    dex::quote_v3_multi_fee(rpc_url, token_in, token_out, amount, V3_FEES).await
}

/// Try V3 via WETH: token_in -> WETH -> token_out
async fn try_v3_via_weth(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount: U256,
) -> Result<Option<QuoteResult>> {
    let weth_addr: Address = WETH.parse()?;

    // token_in -> WETH (cache-aware)
    if let Ok(Some(weth_quote)) =
        get_best_v3_quote_cached(rpc_url, token_in, weth_addr, amount).await
    {
        // WETH -> token_out (cache-aware)
        if let Ok(Some(out_quote)) = get_best_v3_quote_cached(
            rpc_url,
            weth_addr,
            token_out,
            weth_quote.amount_out,
        )
        .await
        {
            return Ok(Some(QuoteResult {
                method: "v3-via-weth".to_string(),
                amount_out: out_quote.amount_out,
                gas_estimate: None,
            }));
        }
    }

    Ok(None)
}

/// Get V4 routing info from DexScreener (quote tokens, pool IDs, and creation block hints)
async fn get_v4_routing_info(
    rpc_url: &str,
    token_address: Address,
) -> Vec<(Address, Option<[u8; 32]>, Option<u64>)> {
    let mut routes = Vec::new();

    // Try to get V4 pairs from DexScreener
    if let Ok(pairs) = dexscreener::get_v4_pairs(token_address).await {
        // Get current block and timestamp for block estimation
        use alloy::providers::Provider;
        if let Ok(rpc_url_parsed) = rpc_url.parse::<reqwest::Url>() {
            let provider = alloy::providers::ProviderBuilder::new().on_http(rpc_url_parsed);

            if let (Ok(current_block), Ok(Some(latest_block_data))) = (
                provider.get_block_number().await,
                provider.get_block_by_number(
                    alloy::rpc::types::BlockNumberOrTag::Latest,
                    alloy::rpc::types::BlockTransactionsKind::Hashes,
                ).await,
            ) {
                let current_timestamp: u64 = latest_block_data.header.timestamp;

                for pair in pairs {
                    // Parse quote token address, pool ID, and estimate creation block
                    if let Some(quote_addr) = pair.quote_token_address() {
                        let pool_id = pair.pool_id();
                        let creation_block = pair.estimate_creation_block(current_block, current_timestamp);

                        // Only add if not already in list
                        if !routes.iter().any(|(addr, _, _)| *addr == quote_addr) {
                            routes.push((quote_addr, pool_id, creation_block));
                        }
                    }
                }
            }
        }
    }

    // Always include WETH and flETH as fallback even if DexScreener doesn't report them
    let weth: Address = WETH.parse().unwrap();
    let fleth: Address = FLETH.parse().unwrap();

    if !routes.iter().any(|(addr, _, _)| *addr == weth) {
        routes.push((weth, None, None));
    }
    if !routes.iter().any(|(addr, _, _)| *addr == fleth) {
        routes.push((fleth, None, None));
    }

    routes
}

/// Try all routes in parallel and return all successful quotes
async fn try_all_routes(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount: U256,
) -> Result<Vec<QuoteResult>> {
    let weth_addr: Address = WETH.parse()?;
    let mut quotes = Vec::new();

    // Get all V4 routing info (quote tokens + pool IDs + block hints) for this token from DexScreener
    let v4_routes = get_v4_routing_info(rpc_url, token_in).await;

    // Build all route tasks to run in parallel
    let mut route_tasks: Vec<tokio::task::JoinHandle<Vec<QuoteResult>>> = Vec::new();

    // 1. V4 direct (token_in -> token_out)
    let rpc_url_clone = rpc_url.to_string();
    route_tasks.push(tokio::spawn(async move {
        if let Ok(Some(pool_info)) = dex::discover_v4_pool_key(&rpc_url_clone, token_in, token_out).await {
            if let Ok(v4_quote) = dex::quote_v4(&rpc_url_clone, token_in, amount, &pool_info).await {
                return vec![v4_quote];
            }
        }
        vec![]
    }));

    // 2. V4 multi-hop routes (all in parallel)
    for (intermediate, pool_id, creation_block) in v4_routes {
        // Skip if intermediate is the same as output
        if intermediate == token_out {
            continue;
        }

        let rpc_url_clone = rpc_url.to_string();
        route_tasks.push(tokio::spawn(async move {
            let mut results = Vec::new();

            // Try token_in -> intermediate via V4
            if let Ok(Some(pool_info)) = dex::v4::discover_v4_pool_key_with_target(
                &rpc_url_clone,
                token_in,
                intermediate,
                pool_id,
                creation_block,
            )
            .await
            {
                if let Ok(v4_quote) = dex::quote_v4(&rpc_url_clone, token_in, amount, &pool_info).await {
                    let intermediate_symbol = if intermediate == weth_addr {
                        "weth"
                    } else if intermediate == FLETH.parse().unwrap_or(Address::ZERO) {
                        "fleth"
                    } else {
                        "token"
                    };

                    // Try both second legs in parallel
                    let rpc1 = rpc_url_clone.clone();
                    let rpc2 = rpc_url_clone.clone();
                    let v4_out_amount = v4_quote.amount_out;

                    let (v4_v4_result, v4_v3_result) = tokio::join!(
                        // V4 -> V4
                        async {
                            if let Ok(Some(out_pool)) =
                                dex::discover_v4_pool_key(&rpc1, intermediate, token_out).await
                            {
                                if let Ok(final_quote) =
                                    dex::quote_v4(&rpc1, intermediate, v4_out_amount, &out_pool).await
                                {
                                    return Some((out_pool, final_quote));
                                }
                            }
                            None
                        },
                        // V4 -> V3
                        async {
                            if let Ok(Some(out_quote)) =
                                get_best_v3_quote_cached(&rpc2, intermediate, token_out, v4_out_amount).await
                            {
                                // Cache intermediate leg
                                let _ = cache::cache_routes(intermediate, token_out, &[out_quote.clone()]);
                                return Some(out_quote);
                            }
                            None
                        }
                    );

                    // Add V4->V4 result
                    if let Some((out_pool, final_quote)) = v4_v4_result {
                        results.push(QuoteResult {
                            method: format!(
                                "v4-{}-v4({},{},{},{},{},{})",
                                intermediate_symbol,
                                pool_info.pool_key.fee,
                                pool_info.pool_key.tick_spacing,
                                pool_info.pool_key.hooks,
                                out_pool.pool_key.fee,
                                out_pool.pool_key.tick_spacing,
                                out_pool.pool_key.hooks
                            ),
                            amount_out: final_quote.amount_out,
                            gas_estimate: None,
                        });
                    }

                    // Add V4->V3 result
                    if let Some(out_quote) = v4_v3_result {
                        results.push(QuoteResult {
                            method: format!(
                                "v4-{}-v3({},{},{},{})",
                                intermediate_symbol,
                                pool_info.pool_key.fee,
                                pool_info.pool_key.tick_spacing,
                                pool_info.pool_key.hooks,
                                out_quote.method.strip_prefix("v3-direct(").and_then(|s| s.strip_suffix(")")).unwrap_or("3000")
                            ),
                            amount_out: out_quote.amount_out,
                            gas_estimate: None,
                        });
                    }
                }
            }

            results
        }));
    }

    // 3. V3 direct
    let rpc_url_clone = rpc_url.to_string();
    route_tasks.push(tokio::spawn(async move {
        if let Ok(Some(quote)) =
            dex::quote_v3_multi_fee(&rpc_url_clone, token_in, token_out, amount, V3_FEES).await
        {
            return vec![quote];
        }
        vec![]
    }));

    // 4. V3 via WETH
    let rpc_url_clone = rpc_url.to_string();
    route_tasks.push(tokio::spawn(async move {
        if let Ok(Some(quote)) = try_v3_via_weth(&rpc_url_clone, token_in, token_out, amount).await {
            return vec![quote];
        }
        vec![]
    }));

    // 5. Aerodrome direct
    let rpc_url_clone = rpc_url.to_string();
    route_tasks.push(tokio::spawn(async move {
        if let Ok(Some(quote)) = dex::quote_aerodrome_best(&rpc_url_clone, token_in, token_out, amount).await {
            return vec![quote];
        }
        vec![]
    }));

    // Wait for all tasks to complete and collect results
    for task in route_tasks {
        if let Ok(results) = task.await {
            quotes.extend(results);
        }
    }

    Ok(quotes)
}

/// CLI command: quote a swap between two tokens
pub async fn quote_swap(amount_str: &str, from_str: &str, to_str: &str) -> Result<()> {
    use crate::constants::resolve_token_address;

    let token_in = resolve_token_address(from_str)
        .ok_or_else(|| anyhow::anyhow!("Invalid token address: {}", from_str))?;
    let token_out = resolve_token_address(to_str)
        .ok_or_else(|| anyhow::anyhow!("Invalid token address: {}", to_str))?;

    // Get token metadata to parse amount correctly
    let rpc_url = get_base_rpc();
    let token_in_meta = dex::get_token_metadata(&rpc_url, token_in).await?;
    let token_out_meta = dex::get_token_metadata(&rpc_url, token_out).await?;

    let amount_in = dex::parse_token_amount(amount_str, token_in_meta.decimals)?;

    println!(
        "Quoting: {} {} -> {}",
        amount_str, token_in_meta.symbol, token_out_meta.symbol
    );
    println!();

    let result = get_quote(token_in, token_out, amount_in, token_in_meta.decimals).await?;

    if !result.quotes.is_empty() {
        println!("Routes:");
        for quote in &result.quotes {
            let amount_out = dex::format_token_amount(quote.amount_out, token_out_meta.decimals);
            println!(
                "  {}: {} {}",
                quote.method, amount_out, token_out_meta.symbol
            );
        }
        println!();

        if let Some(best) = &result.best_quote {
            let best_out = dex::format_token_amount(best.amount_out, token_out_meta.decimals);
            println!(
                "Best:  {} {} ({})",
                best_out, token_out_meta.symbol, best.method
            );

            // Calculate effective price
            let amount_in_f64 = dex::format_token_amount(amount_in, token_in_meta.decimals);
            let effective_price = best_out / amount_in_f64;
            println!(
                "Price: 1 {} = {:.6} {}",
                token_in_meta.symbol, effective_price, token_out_meta.symbol
            );
        }
    } else {
        println!(
            "No routes found between {} and {}",
            token_in_meta.symbol, token_out_meta.symbol
        );
        println!("This pair may not have active liquidity pools.");
    }

    Ok(())
}

/// CLI command: quote all portfolio positions
pub async fn portfolio_quotes() -> Result<()> {
    println!("Portfolio quoting not yet implemented - requires trades DB");
    Ok(())
}

/// CLI command: refresh all cached routes
pub async fn refresh_all_cached_routes(test_amount_str: &str) -> Result<()> {
    use crate::constants::resolve_token_address;

    println!("Refreshing all cached routes...\n");

    // Load current cache
    let cache = cache::load_route_cache()?;
    let pairs: Vec<(Address, Address)> = cache
        .keys()
        .filter_map(|key| {
            let parts: Vec<&str> = key.split(':').collect();
            if parts.len() == 2 {
                let token_in = parts[0].parse().ok()?;
                let token_out = parts[1].parse().ok()?;
                Some((token_in, token_out))
            } else {
                None
            }
        })
        .collect();

    if pairs.is_empty() {
        println!("No cached routes found. Cache is empty.");
        return Ok(());
    }

    println!("Found {} token pairs in cache\n", pairs.len());

    let rpc_url = get_base_rpc();
    let mut refreshed = 0;
    let mut failed = 0;

    for (i, (token_in, token_out)) in pairs.iter().enumerate() {
        // Get token metadata
        let token_in_meta = match dex::get_token_metadata(&rpc_url, *token_in).await {
            Ok(meta) => meta,
            Err(e) => {
                println!(
                    "[{}/{}] Failed to get metadata for {}: {}",
                    i + 1,
                    pairs.len(),
                    token_in,
                    e
                );
                failed += 1;
                continue;
            }
        };

        let token_out_meta = match dex::get_token_metadata(&rpc_url, *token_out).await {
            Ok(meta) => meta,
            Err(e) => {
                println!(
                    "[{}/{}] Failed to get metadata for {}: {}",
                    i + 1,
                    pairs.len(),
                    token_out,
                    e
                );
                failed += 1;
                continue;
            }
        };

        // Parse test amount with correct decimals
        let amount = match dex::parse_token_amount(test_amount_str, token_in_meta.decimals) {
            Ok(amt) => amt,
            Err(e) => {
                println!(
                    "[{}/{}] Failed to parse amount for {} -> {}: {}",
                    i + 1,
                    pairs.len(),
                    token_in_meta.symbol,
                    token_out_meta.symbol,
                    e
                );
                failed += 1;
                continue;
            }
        };

        print!(
            "[{}/{}] Refreshing {} -> {}... ",
            i + 1,
            pairs.len(),
            token_in_meta.symbol,
            token_out_meta.symbol
        );

        // Get fresh quotes (force full route evaluation)
        match try_all_routes(&rpc_url, *token_in, *token_out, amount).await {
            Ok(quotes) if !quotes.is_empty() => {
                // Cache the fresh routes
                let _ = cache::cache_routes(*token_in, *token_out, &quotes);
                let best = quotes.iter().max_by_key(|q| q.amount_out).unwrap();
                println!(
                    "OK - {} routes cached (best: {})",
                    quotes.len(),
                    best.method
                );
                refreshed += 1;
            }
            Ok(_) => {
                println!("WARN - No routes found");
                failed += 1;
            }
            Err(e) => {
                println!("ERROR: {}", e);
                failed += 1;
            }
        }

        // Small delay to avoid rate limiting
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    println!("\n============================================");
    println!("Refreshed: {}", refreshed);
    println!("Failed:    {}", failed);
    println!("Total:     {}", pairs.len());
    println!("============================================");

    Ok(())
}
