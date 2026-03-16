//! Fork-based integration tests for the trade/approval flow.
//!
//! These tests fork Base mainnet via Anvil and exercise real Permit2/ERC20 contracts.
//! They require BASE_RPC_URL to be set and are gated with #[ignore].
//!
//! Run with: BASE_RPC_URL=<url> cargo test -- --ignored

use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use alloy::sol_types::SolCall;
use basalt::wallet::Wallet;

// Known addresses
const PERMIT2: &str = "0x000000000022D473030F116dDEE9F6B43aC78BA3";
const UNIVERSAL_ROUTER: &str = "0x6ff5693b99212da76ad316178a184ab56d299b43";
const USDC: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";

// A known USDC whale on Base (Circle's address — holds large USDC balance)
const USDC_WHALE: &str = "0x3304E22DDaa22bCdC5fCa2269b418046aE7b566A";

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function allowance(address owner, address spender) external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
    }
}

sol! {
    #[sol(rpc)]
    interface IPermit2Read {
        function allowance(address owner, address token, address spender) external view returns (uint160 amount, uint48 expiration, uint48 nonce);
    }
}

fn get_fork_rpc() -> String {
    std::env::var("BASE_RPC_URL").expect("BASE_RPC_URL must be set for fork tests")
}

fn spawn_fork_anvil() -> AnvilInstance {
    let rpc = get_fork_rpc();
    Anvil::new().fork(rpc).spawn()
}

/// Fund test wallet with an ERC20 token by impersonating a whale.
async fn fund_with_token(
    provider: &impl Provider,
    token: Address,
    whale: Address,
    recipient: Address,
    amount: U256,
) {
    // Give whale ETH for gas (in case they don't have any)
    provider
        .raw_request::<_, ()>(
            "anvil_setBalance".into(),
            &(whale, U256::from(10u64).pow(U256::from(18u64))), // 1 ETH
        )
        .await
        .expect("setBalance failed");

    // Impersonate the whale
    provider
        .raw_request::<_, ()>("anvil_impersonateAccount".into(), &[whale])
        .await
        .expect("impersonate failed");

    // Build transfer calldata
    let calldata = IERC20::transferCall {
        to: recipient,
        amount,
    }
    .abi_encode();

    // Send transfer as the whale (using a plain provider, no wallet needed for impersonated accounts)
    let tx = alloy::rpc::types::TransactionRequest::default()
        .from(whale)
        .to(token)
        .input(Bytes::from(calldata).into());

    let pending = provider.send_transaction(tx).await.expect("transfer tx failed");
    let receipt = pending.get_receipt().await.expect("transfer receipt failed");
    assert!(receipt.status(), "token transfer from whale should succeed");

    // Stop impersonating
    provider
        .raw_request::<_, ()>("anvil_stopImpersonatingAccount".into(), &[whale])
        .await
        .expect("stop impersonating failed");

    // Verify balance
    let erc20 = IERC20::new(token, provider);
    let balance = erc20.balanceOf(recipient).call().await.expect("balanceOf failed");
    assert!(balance >= amount, "recipient should have at least {amount} tokens");
}

#[tokio::test]
#[ignore]
async fn fork_test_erc20_approval_to_permit2() {
    let anvil = spawn_fork_anvil();
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = Wallet::from_signer(signer);

    let usdc: Address = USDC.parse().unwrap();
    let permit2: Address = PERMIT2.parse().unwrap();

    // Fund wallet with a small amount of USDC (needed so the approval path runs)
    let provider = ProviderBuilder::new().connect_http(anvil.endpoint().parse().unwrap());
    fund_with_token(&provider, usdc, USDC_WHALE.parse().unwrap(), wallet.address, U256::from(1_000_000u64)).await;

    // Run ensure_approvals
    basalt::trade::ensure_approvals(&anvil.endpoint(), &wallet, usdc, "USDC")
        .await
        .expect("ensure_approvals should succeed");

    // Verify ERC20 allowance to Permit2 is MAX
    let erc20 = IERC20::new(usdc, &provider);
    let allowance = erc20.allowance(wallet.address, permit2).call().await.unwrap();
    assert_eq!(allowance, U256::MAX, "ERC20 allowance to Permit2 should be MAX");
}

#[tokio::test]
#[ignore]
async fn fork_test_permit2_approval_to_router() {
    let anvil = spawn_fork_anvil();
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = Wallet::from_signer(signer);

    let usdc: Address = USDC.parse().unwrap();
    let permit2: Address = PERMIT2.parse().unwrap();
    let router: Address = UNIVERSAL_ROUTER.parse().unwrap();

    let provider = ProviderBuilder::new().connect_http(anvil.endpoint().parse().unwrap());
    fund_with_token(&provider, usdc, USDC_WHALE.parse().unwrap(), wallet.address, U256::from(1_000_000u64)).await;

    basalt::trade::ensure_approvals(&anvil.endpoint(), &wallet, usdc, "USDC")
        .await
        .expect("ensure_approvals should succeed");

    // Check Permit2 allowance to Router
    let permit2_contract = IPermit2Read::new(permit2, &provider);
    let result = permit2_contract
        .allowance(wallet.address, usdc, router)
        .call()
        .await
        .unwrap();

    // Amount should be max uint160
    assert_eq!(result.amount, alloy::primitives::Uint::<160, 3>::MAX);

    // Expiration should be roughly 30 days from now
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expiration: u64 = result.expiration.to::<u64>();
    let twenty_nine_days = 29 * 24 * 3600;
    let thirty_one_days = 31 * 24 * 3600;
    assert!(
        expiration > now + twenty_nine_days && expiration < now + thirty_one_days,
        "expiration {expiration} should be ~30 days from now ({now})"
    );
}

#[tokio::test]
#[ignore]
async fn fork_test_approvals_are_idempotent() {
    let anvil = spawn_fork_anvil();
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = Wallet::from_signer(signer);

    let usdc: Address = USDC.parse().unwrap();

    let provider = ProviderBuilder::new().connect_http(anvil.endpoint().parse().unwrap());
    fund_with_token(&provider, usdc, USDC_WHALE.parse().unwrap(), wallet.address, U256::from(1_000_000u64)).await;

    // First call sets approvals
    basalt::trade::ensure_approvals(&anvil.endpoint(), &wallet, usdc, "USDC")
        .await
        .expect("first ensure_approvals should succeed");

    // Second call should skip (already approved) and not error
    basalt::trade::ensure_approvals(&anvil.endpoint(), &wallet, usdc, "USDC")
        .await
        .expect("second ensure_approvals should succeed (idempotent)");
}

/// End-to-end swap test: fund wallet with USDC → approve via Permit2 → swap USDC→WETH
/// via Universal Router V3 command.
///
/// Uses the known USDC/WETH V3 pool on Base (fee tier 3000, pool 0x6c56...1372).
/// Constructs calldata directly to isolate the execution path from the quoting layer.
#[tokio::test]
#[ignore]
async fn fork_test_full_swap_usdc_to_weth() {
    let anvil = spawn_fork_anvil();
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = Wallet::from_signer(signer);

    let usdc: Address = USDC.parse().unwrap();
    let weth: Address = "0x4200000000000000000000000000000000000006".parse().unwrap();
    let router: Address = UNIVERSAL_ROUTER.parse().unwrap();

    let provider = ProviderBuilder::new().connect_http(anvil.endpoint().parse().unwrap());

    // Fund wallet with 1 USDC (1_000_000 = 1e6)
    let amount_in = U256::from(1_000_000u64);
    fund_with_token(&provider, usdc, USDC_WHALE.parse().unwrap(), wallet.address, amount_in).await;

    // Ensure approvals (ERC20 → Permit2 → Router)
    basalt::trade::ensure_approvals(&anvil.endpoint(), &wallet, usdc, "USDC")
        .await
        .expect("ensure_approvals should succeed");

    // Build V3 swap calldata: USDC → WETH, fee tier 3000 (the actual pool fee), 0 minOut for test
    let deadline = U256::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300,
    );
    let calldata = basalt::universal_router::encode_v3_single(
        usdc, weth, 3000, amount_in, U256::ZERO, deadline,
    );

    // Submit swap transaction
    let result = basalt::executor::send_transaction(
        &anvil.endpoint(),
        &wallet,
        basalt::executor::TransactionRequest {
            to: router,
            calldata: Bytes::from(calldata),
            value: U256::ZERO,
        },
        120,
    )
    .await
    .expect("swap transaction should succeed on fork");

    assert!(result.receipt.status(), "swap transaction should not revert");
    assert!(result.gas_used > 0, "gas should be consumed");

    // Verify USDC balance decreased by amount_in
    let usdc_contract = IERC20::new(usdc, &provider);
    let usdc_balance = usdc_contract.balanceOf(wallet.address).call().await.unwrap();
    assert_eq!(usdc_balance, U256::ZERO, "all USDC should have been swapped");

    // Verify we received some WETH
    sol! {
        #[sol(rpc)]
        interface IWETH {
            function balanceOf(address account) external view returns (uint256);
        }
    }
    let weth_contract = IWETH::new(weth, &provider);
    let weth_balance = weth_contract.balanceOf(wallet.address).call().await.unwrap();
    assert!(weth_balance > U256::ZERO, "should have received WETH from swap");
}

/// End-to-end V4 swap test: sell ROBOTMONEY for WETH via V4 pool with hooks.
///
/// Pool: WETH/ROBOTMONEY, fee=8388608 (dynamic), tickSpacing=200,
///       hooks=0xbb7784a4d481184283ed89619a3e3ed143e1adc0
///       poolId=0xcece56fd6eb8fcbc6c45af8181bfe71ea6057770630490cac36dbbc4aa27a4a6
/// Direction: selling ROBOTMONEY (currency1) for WETH (currency0) → zero_for_one = false
#[tokio::test]
#[ignore]
async fn fork_test_v4_swap_to_weth() {
    let anvil = spawn_fork_anvil();
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = Wallet::from_signer(signer);

    let weth: Address = "0x4200000000000000000000000000000000000006".parse().unwrap();
    let token: Address = "0x65021a79AeEF22b17cdc1B768f5e79a8618bEbA3".parse().unwrap();
    let router: Address = UNIVERSAL_ROUTER.parse().unwrap();

    let provider = ProviderBuilder::new().connect_http(anvil.endpoint().parse().unwrap());

    let whale: Address = "0xfbc2cc30f0674ed0244ee1f0ba7864423230c9d6".parse().unwrap();

    // Fund wallet with 1 token (18 decimals)
    let amount_in = U256::from(10u64).pow(U256::from(18u64));
    fund_with_token(&provider, token, whale, wallet.address, amount_in).await;

    // Ensure approvals (ERC20 → Permit2 → Router)
    basalt::trade::ensure_approvals(&anvil.endpoint(), &wallet, token, "ROBOTMONEY")
        .await
        .expect("ensure_approvals should succeed");

    // Use the V4 pool with dynamic fee and hooks (highest liquidity)
    // poolId: 0xcece56fd6eb8fcbc6c45af8181bfe71ea6057770630490cac36dbbc4aa27a4a6
    let pool_key = basalt::types::V4PoolKey {
        currency0: weth,
        currency1: token,
        fee: 8388608,      // dynamic fee flag
        tick_spacing: 200,
        hooks: "0xbb7784a4d481184283ed89619a3e3ed143e1adc0".parse().unwrap(),
    };

    let pool_info = basalt::types::V4PoolInfo {
        pool_key: pool_key.clone(),
        pool_id: [0u8; 32],
        sqrt_price_x96: alloy::primitives::Uint::<160, 3>::ZERO,
        liquidity: 0,
        zero_for_one: false, // selling currency1 (token) for currency0 (WETH)
    };

    // Verify pool is quotable
    let quote = basalt::dex::v4::quote_v4(&anvil.endpoint(), token, amount_in, &pool_info)
        .await
        .expect("V4 quoter should return a quote — pool must be active");
    eprintln!("[test] V4 quote: 1 ROBOTMONEY -> {} WETH (raw)", quote.amount_out);
    assert!(quote.amount_out > U256::ZERO, "quote should return nonzero output");

    // Build V4 swap calldata
    let zero_for_one = false;
    let deadline = U256::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300,
    );
    let calldata = basalt::universal_router::encode_v4_single(
        &pool_info.pool_key,
        zero_for_one,
        amount_in.to::<u128>(),
        0u128, // minAmountOut = 0 for test
        deadline,
    );

    // Submit swap transaction
    let result = basalt::executor::send_transaction(
        &anvil.endpoint(),
        &wallet,
        basalt::executor::TransactionRequest {
            to: router,
            calldata: Bytes::from(calldata),
            value: U256::ZERO,
        },
        120,
    )
    .await
    .expect("V4 swap transaction should succeed on fork");

    assert!(result.receipt.status(), "V4 swap transaction should not revert");
    assert!(result.gas_used > 0, "gas should be consumed");

    // Verify token balance is zero (all swapped)
    let token_contract = IERC20::new(token, &provider);
    let final_balance = token_contract.balanceOf(wallet.address).call().await.unwrap();
    assert_eq!(final_balance, U256::ZERO, "all tokens should have been swapped");

    // Verify we received some WETH
    let weth_contract = IERC20::new(weth, &provider);
    let weth_balance = weth_contract.balanceOf(wallet.address).call().await.unwrap();
    assert!(weth_balance > U256::ZERO, "should have received WETH from V4 swap");
}

