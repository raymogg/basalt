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

    // V4 via WETH (multi-hop, still needs discovery)
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

/// Try V3 via WETH: token_in -> WETH -> token_out
async fn try_v3_via_weth(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount: U256,
) -> Result<Option<QuoteResult>> {
    let weth_addr: Address = WETH.parse()?;

    // token_in -> WETH
    if let Ok(Some(weth_quote)) =
        dex::quote_v3_multi_fee(rpc_url, token_in, weth_addr, amount, V3_FEES).await
    {
        // WETH -> token_out
        if let Ok(Some(out_quote)) = dex::quote_v3_multi_fee(
            rpc_url,
            weth_addr,
            token_out,
            weth_quote.amount_out,
            V3_FEES,
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

    // 1. Try V4 direct (token_in -> token_out)
    if let Some(pool_info) = dex::discover_v4_pool_key(rpc_url, token_in, token_out).await? {
        if let Ok(v4_quote) = dex::quote_v4(rpc_url, token_in, amount, &pool_info).await {
            quotes.push(v4_quote);
        }
    }

    // 2. Try V4 multi-hop routes using DexScreener-discovered quote tokens and pool IDs
    // Get all V4 routing info (quote tokens + pool IDs + block hints) for this token from DexScreener
    let v4_routes = get_v4_routing_info(rpc_url, token_in).await;

    for (intermediate, pool_id, creation_block) in v4_routes {
        // Skip if intermediate is the same as output
        if intermediate == token_out {
            continue;
        }

        // Try token_in -> intermediate via V4 with target pool ID and creation block hint
        if let Some(pool_info) = dex::v4::discover_v4_pool_key_with_target(
            rpc_url,
            token_in,
            intermediate,
            pool_id,
            creation_block,
        )
        .await?
        {
            if let Ok(v4_quote) = dex::quote_v4(rpc_url, token_in, amount, &pool_info).await {
                let intermediate_symbol = if intermediate == weth_addr {
                    "weth"
                } else if intermediate == FLETH.parse().unwrap_or(Address::ZERO) {
                    "fleth"
                } else {
                    "token"
                };

                // Try intermediate -> token_out via V4
                if let Some(out_pool) =
                    dex::discover_v4_pool_key(rpc_url, intermediate, token_out).await?
                {
                    if let Ok(final_quote) =
                        dex::quote_v4(rpc_url, intermediate, v4_quote.amount_out, &out_pool).await
                    {
                        quotes.push(QuoteResult {
                            method: format!("v4-{}-v4", intermediate_symbol),
                            amount_out: final_quote.amount_out,
                            gas_estimate: None,
                        });
                    }
                }

                // Try intermediate -> token_out via V3
                if let Ok(Some(out_quote)) = dex::quote_v3_multi_fee(
                    rpc_url,
                    intermediate,
                    token_out,
                    v4_quote.amount_out,
                    V3_FEES,
                )
                .await
                {
                    quotes.push(QuoteResult {
                        method: format!("v4-{}-v3", intermediate_symbol),
                        amount_out: out_quote.amount_out,
                        gas_estimate: None,
                    });
                }
            }
        }
    }

    // 5. Try V3 direct
    if let Ok(Some(quote)) =
        dex::quote_v3_multi_fee(rpc_url, token_in, token_out, amount, V3_FEES).await
    {
        quotes.push(quote);
    }

    // 6. Try V3 via WETH
    if let Ok(Some(quote)) = try_v3_via_weth(rpc_url, token_in, token_out, amount).await {
        quotes.push(quote);
    }

    // 7. Try Aerodrome direct
    if let Ok(Some(quote)) = dex::quote_aerodrome_best(rpc_url, token_in, token_out, amount).await {
        quotes.push(quote);
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
