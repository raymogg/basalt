use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::{Bytes, U256};
use alloy::signers::local::PrivateKeySigner;
use basalt::executor::{self, TransactionRequest};
use basalt::wallet::Wallet;

fn setup_anvil() -> (AnvilInstance, Wallet) {
    let anvil = Anvil::new().spawn();
    let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let wallet = Wallet::from_signer(signer);
    (anvil, wallet)
}

#[tokio::test]
async fn test_send_simple_eth_transfer() {
    let (anvil, wallet) = setup_anvil();
    let recipient: alloy::primitives::Address = anvil.addresses()[1];

    let tx = TransactionRequest {
        to: recipient,
        calldata: Bytes::new(),
        value: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
    };

    let result = executor::send_transaction(&anvil.endpoint(), &wallet, tx, 30).await;
    let result = result.expect("transaction should succeed");

    assert!(result.gas_used > 0, "gas_used should be populated");
    assert!(result.effective_gas_price > 0, "effective_gas_price should be populated");
}

#[tokio::test]
async fn test_send_transaction_receipt_success() {
    let (anvil, wallet) = setup_anvil();
    let recipient = anvil.addresses()[1];

    let tx = TransactionRequest {
        to: recipient,
        calldata: Bytes::new(),
        value: U256::from(1_000u64),
    };

    let result = executor::send_transaction(&anvil.endpoint(), &wallet, tx, 30)
        .await
        .expect("transaction should succeed");

    assert!(result.receipt.status(), "receipt status should be true");
}

#[tokio::test]
async fn test_send_transaction_with_calldata() {
    let (anvil, wallet) = setup_anvil();
    let recipient = anvil.addresses()[1];

    // Send to an EOA with non-empty calldata — EOAs ignore calldata
    let tx = TransactionRequest {
        to: recipient,
        calldata: Bytes::from(vec![0xde, 0xad, 0xbe, 0xef]),
        value: U256::from(1_000u64),
    };

    let result = executor::send_transaction(&anvil.endpoint(), &wallet, tx, 30)
        .await
        .expect("transaction with calldata to EOA should succeed");

    assert!(result.receipt.status());
}

#[tokio::test]
async fn test_send_transaction_to_self() {
    let (anvil, wallet) = setup_anvil();

    let tx = TransactionRequest {
        to: wallet.address,
        calldata: Bytes::new(),
        value: U256::ZERO,
    };

    let result = executor::send_transaction(&anvil.endpoint(), &wallet, tx, 30)
        .await
        .expect("self-transfer with zero value should succeed");

    assert!(result.receipt.status());
}
