use crate::constants::V3_QUOTER_V2;
use crate::types::QuoteResult;
use alloy::primitives::{Address, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;
use anyhow::Result;

// V3 QuoterV2 interface
sol! {
    #[sol(rpc)]
    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }

        function quoteExactInputSingle(QuoteExactInputSingleParams calldata params) external returns (
            uint256 amountOut,
            uint160 sqrtPriceX96After,
            uint32 initializedTicksCrossed,
            uint256 gasEstimate
        );
    }
}

/// Get quote from V3 QuoterV2
pub async fn quote_v3(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    fee: u32,
) -> Result<QuoteResult> {
    let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let quoter = IQuoterV2::new(V3_QUOTER_V2.parse()?, provider);

    let params = IQuoterV2::QuoteExactInputSingleParams {
        tokenIn: token_in,
        tokenOut: token_out,
        amountIn: amount_in,
        fee: alloy::primitives::Uint::<24, 1>::from(fee),
        sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
    };

    match quoter.quoteExactInputSingle(params).call().await {
        Ok(result) => Ok(QuoteResult {
            method: format!("v3-direct({})", fee),
            display_name: String::new(),
            amount_out: result.amountOut,
            gas_estimate: Some(result.gasEstimate),
            pool_id: None,
            calldata: None,
        }),
        Err(e) => {
            anyhow::bail!("V3 quote failed: {}", e);
        }
    }
}

/// Try multiple V3 fee tiers and return the best quote (batched via parallel execution)
pub async fn quote_v3_multi_fee(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    fees: &[u32],
) -> Result<Option<QuoteResult>> {
    // Execute all fee tier queries in parallel
    let mut tasks = Vec::new();

    for &fee in fees {
        let rpc_url = rpc_url.to_string();
        let task = tokio::spawn(async move {
            quote_v3(&rpc_url, token_in, token_out, amount_in, fee).await
        });
        tasks.push(task);
    }

    // Collect results and find best quote
    let mut best_quote: Option<QuoteResult> = None;

    for task in tasks {
        if let Ok(Ok(quote)) = task.await {
            if let Some(ref current_best) = best_quote {
                if quote.amount_out > current_best.amount_out {
                    best_quote = Some(quote);
                }
            } else {
                best_quote = Some(quote);
            }
        }
    }

    Ok(best_quote)
}
