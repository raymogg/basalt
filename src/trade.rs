use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::{bail, Context, Result};

use crate::constants::{get_base_rpc, resolve_token_address};
use crate::dex;
use crate::executor;
use crate::universal_router;
use crate::wallet::Wallet;

// Permit2 canonical address (same on all chains)
const PERMIT2: &str = "0x000000000022D473030F116dDEE9F6B43aC78BA3";

// Minimal ERC20 interface for approve/allowance
sol! {
    #[sol(rpc)]
    interface IERC20Approval {
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
    }
}

// Permit2 allowance interface
sol! {
    #[sol(rpc)]
    interface IPermit2 {
        function allowance(address owner, address token, address spender) external view returns (uint160 amount, uint48 expiration, uint48 nonce);
        function approve(address token, address spender, uint160 amount, uint48 expiration) external;
    }
}

/// Execute a swap: quote -> approve -> submit transaction
pub async fn execute_swap(
    amount_str: &str,
    from_str: &str,
    to_str: &str,
    skip_confirmation: bool,
) -> Result<()> {
    let rpc_url = get_base_rpc();

    // Load wallet
    let wallet = Wallet::from_env()?;
    eprintln!("[wallet] Address: {}", wallet.address);

    // Resolve tokens
    let token_in = resolve_token_address(from_str)
        .context(format!("Cannot resolve input token: {}", from_str))?;
    let token_out = resolve_token_address(to_str)
        .context(format!("Cannot resolve output token: {}", to_str))?;

    // Get token metadata
    let meta_in = dex::get_token_metadata(&rpc_url, token_in).await?;
    let meta_out = dex::get_token_metadata(&rpc_url, token_out).await?;

    // Parse amount
    let amount_in = dex::parse_token_amount(amount_str, meta_in.decimals)?;

    // Get quote
    eprintln!("[quote] Getting best route for {} {} -> {} ...",
        amount_str, meta_in.symbol, meta_out.symbol);
    let quote_result = crate::quote::get_quote(token_in, token_out, amount_in, meta_in.decimals).await?;

    let best = match quote_result.best_quote {
        Some(q) => q,
        None => bail!("No route found for {} -> {}", meta_in.symbol, meta_out.symbol),
    };

    let calldata_hex = match &best.calldata {
        Some(cd) => cd.clone(),
        None => bail!("Best route has no calldata (unsupported route type: {})", best.method),
    };

    let amount_out_f = dex::format_token_amount(best.amount_out, meta_out.decimals);
    let amount_in_f: f64 = amount_str.parse().unwrap_or(0.0);

    // Display quote
    println!("\n  Swap: {} {} -> {} {}", amount_in_f, meta_in.symbol, amount_out_f, meta_out.symbol);
    println!("  Route: {}", best.method);
    println!("  Slippage: 0.5%");
    println!("  Router: {}", universal_router::UNIVERSAL_ROUTER);

    // Confirmation
    if !skip_confirmation {
        eprint!("\n  Proceed? [y/N] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("  Cancelled.");
            return Ok(());
        }
    }

    // Ensure approvals (ERC20 -> Permit2 -> Router)
    ensure_approvals(&rpc_url, &wallet, token_in, &meta_in.symbol).await?;

    // Submit swap
    let calldata_bytes = alloy::primitives::hex::decode(calldata_hex.strip_prefix("0x").unwrap_or(&calldata_hex))?;
    let router_address: Address = universal_router::UNIVERSAL_ROUTER.parse()?;

    eprintln!("[swap] Submitting swap transaction...");
    let result = executor::send_transaction(
        &rpc_url,
        &wallet,
        executor::TransactionRequest {
            to: router_address,
            calldata: Bytes::from(calldata_bytes),
            value: U256::ZERO,
        },
        120,
    )
    .await?;

    let success = result.receipt.status();
    let tx_hash = alloy::primitives::hex::encode(result.tx_hash);
    let gas_cost_eth = (result.gas_used as f64) * (result.effective_gas_price as f64) / 1e18;

    println!("\n  {}", if success { "Swap successful!" } else { "Swap FAILED!" });
    println!("  Tx: https://basescan.org/tx/0x{}", tx_hash);
    println!("  Gas used: {} ({:.6} ETH)", result.gas_used, gas_cost_eth);

    if !success {
        bail!("Swap transaction reverted");
    }

    Ok(())
}

/// Ensure ERC20 approval to Permit2 and Permit2 approval to the Universal Router.
async fn ensure_approvals(
    rpc_url: &str,
    wallet: &Wallet,
    token: Address,
    symbol: &str,
) -> Result<()> {
    let permit2: Address = PERMIT2.parse()?;
    let router: Address = universal_router::UNIVERSAL_ROUTER.parse()?;

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    // Step A: Check ERC20 allowance to Permit2
    let erc20 = IERC20Approval::new(token, &provider);
    let allowance = erc20.allowance(wallet.address, permit2).call().await?;
    if allowance < U256::from(u128::MAX) {
        eprintln!("[approve] Approving {} to Permit2...", symbol);
        let calldata = IERC20Approval::approveCall {
            spender: permit2,
            amount: U256::MAX,
        }.abi_encode();
        let result = executor::send_transaction(
            rpc_url,
            wallet,
            executor::TransactionRequest {
                to: token,
                calldata: Bytes::from(calldata),
                value: U256::ZERO,
            },
            60,
        )
        .await
        .context("ERC20 approve to Permit2 failed")?;
        if !result.receipt.status() {
            bail!("ERC20 approve transaction reverted");
        }
        eprintln!("[approve] {} approved to Permit2", symbol);
    }

    // Step B: Check Permit2 allowance to Router
    let permit2_contract = IPermit2::new(permit2, &provider);
    let result = permit2_contract
        .allowance(wallet.address, token, router)
        .call()
        .await?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let needs_permit2_approve = result.amount.is_zero() || (result.expiration < alloy::primitives::Uint::<48, 1>::from(now));

    if needs_permit2_approve {
        eprintln!("[approve] Approving {} on Permit2 for Router...", symbol);
        let max_uint160 = alloy::primitives::Uint::<160, 3>::MAX;
        let thirty_days = alloy::primitives::Uint::<48, 1>::from(now + 30 * 24 * 3600);
        let calldata = IPermit2::approveCall {
            token,
            spender: router,
            amount: max_uint160,
            expiration: thirty_days,
        }.abi_encode();
        let result = executor::send_transaction(
            rpc_url,
            wallet,
            executor::TransactionRequest {
                to: permit2,
                calldata: Bytes::from(calldata),
                value: U256::ZERO,
            },
            60,
        )
        .await
        .context("Permit2 approve failed")?;
        if !result.receipt.status() {
            bail!("Permit2 approve transaction reverted");
        }
        eprintln!("[approve] Permit2 approval set for {}", symbol);
    }

    Ok(())
}
