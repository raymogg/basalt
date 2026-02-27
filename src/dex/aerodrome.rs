use crate::constants::{AERO_DEFAULT_FACTORY, AERO_ROUTER};
use crate::types::QuoteResult;
use alloy::primitives::{Address, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;
use anyhow::Result;

// Aerodrome Router interface
sol! {
    #[sol(rpc)]
    interface IAeroRouter {
        struct Route {
            address from;
            address to;
            bool stable;
            address factory;
        }

        function getAmountsOut(uint256 amountIn, Route[] memory routes) external view returns (uint256[] memory amounts);
    }
}

/// Get quote from Aerodrome Router
pub async fn quote_aerodrome(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    stable: bool,
) -> Result<QuoteResult> {
    let provider = ProviderBuilder::new().on_http(rpc_url.parse()?);
    let router = IAeroRouter::new(AERO_ROUTER.parse()?, provider);

    let route = IAeroRouter::Route {
        from: token_in,
        to: token_out,
        stable,
        factory: AERO_DEFAULT_FACTORY.parse()?,
    };

    let routes = vec![route];

    match router.getAmountsOut(amount_in, routes).call().await {
        Ok(amounts) => {
            if amounts.amounts.len() >= 2 && amounts.amounts[1] > U256::ZERO {
                Ok(QuoteResult {
                    method: format!("aerodrome-{}", if stable { "stable" } else { "volatile" }),
                    amount_out: amounts.amounts[1],
                    gas_estimate: None,
                })
            } else {
                anyhow::bail!("Aerodrome returned zero output");
            }
        }
        Err(e) => {
            anyhow::bail!("Aerodrome quote failed: {}", e);
        }
    }
}

/// Try both stable and volatile pools, return the best
pub async fn quote_aerodrome_best(
    rpc_url: &str,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
) -> Result<Option<QuoteResult>> {
    let mut best_quote: Option<QuoteResult> = None;

    for stable in [false, true] {
        if let Ok(quote) = quote_aerodrome(rpc_url, token_in, token_out, amount_in, stable).await {
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
