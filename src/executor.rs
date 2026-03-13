use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionReceipt;
use anyhow::{Context, Result};

use crate::wallet::Wallet;

pub struct TransactionRequest {
    pub to: Address,
    pub calldata: Bytes,
    pub value: U256,
}

pub struct TransactionResult {
    pub tx_hash: alloy::primitives::TxHash,
    pub receipt: TransactionReceipt,
    pub gas_used: u64,
    pub effective_gas_price: u128,
}

/// Send a transaction and wait for its receipt.
/// Alloy's fillers handle gas estimation, nonce, and chain ID automatically.
pub async fn send_transaction(
    rpc_url: &str,
    wallet: &Wallet,
    tx: TransactionRequest,
    timeout_secs: u64,
) -> Result<TransactionResult> {
    let provider = ProviderBuilder::new()
        .wallet(wallet.ethereum_wallet())
        .connect_http(rpc_url.parse()?);

    let tx_request = alloy::rpc::types::TransactionRequest::default()
        .to(tx.to)
        .input(tx.calldata.into())
        .value(tx.value);

    let pending = provider
        .send_transaction(tx_request)
        .await
        .context("Failed to send transaction")?;

    let tx_hash = *pending.tx_hash();
    eprintln!("[tx] Sent: 0x{}", alloy::primitives::hex::encode(tx_hash));

    let receipt = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        pending.get_receipt(),
    )
    .await
    .context("Transaction confirmation timed out")?
    .context("Failed to get transaction receipt")?;

    let gas_used = receipt.gas_used;
    let effective_gas_price = receipt.effective_gas_price;

    Ok(TransactionResult {
        tx_hash,
        receipt,
        gas_used,
        effective_gas_price,
    })
}
