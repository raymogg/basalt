use alloy::signers::local::PrivateKeySigner;
use basalt::wallet::Wallet;
use std::sync::Mutex;

// Anvil default key #0
const ANVIL_KEY_0: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const ANVIL_ADDR_0: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

// Serialize env-var tests to avoid races
static ENV_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn test_wallet_from_env_valid_key() {
    let _guard = ENV_MUTEX.lock().unwrap();
    std::env::set_var("BASALT_PRIVATE_KEY", ANVIL_KEY_0);

    let wallet = Wallet::from_env().expect("should load valid key");
    let expected: alloy::primitives::Address = ANVIL_ADDR_0.parse().unwrap();
    assert_eq!(wallet.address, expected);

    std::env::remove_var("BASALT_PRIVATE_KEY");
}

#[test]
fn test_wallet_from_env_missing_key() {
    let _guard = ENV_MUTEX.lock().unwrap();
    std::env::remove_var("BASALT_PRIVATE_KEY");

    let result = Wallet::from_env();
    let err = result.err().expect("should fail when env var is missing");
    let err_msg = format!("{}", err);
    assert!(err_msg.contains("BASALT_PRIVATE_KEY"), "error should mention env var: {err_msg}");
}

#[test]
fn test_wallet_from_env_invalid_key() {
    let _guard = ENV_MUTEX.lock().unwrap();
    std::env::set_var("BASALT_PRIVATE_KEY", "not-a-valid-hex-key");

    let result = Wallet::from_env();
    assert!(result.is_err());

    std::env::remove_var("BASALT_PRIVATE_KEY");
}

#[test]
fn test_wallet_from_signer() {
    let signer: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let wallet = Wallet::from_signer(signer);

    let expected: alloy::primitives::Address = ANVIL_ADDR_0.parse().unwrap();
    assert_eq!(wallet.address, expected);
}

#[test]
fn test_wallet_address_deterministic() {
    let signer1: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();
    let signer2: PrivateKeySigner = ANVIL_KEY_0.parse().unwrap();

    let wallet1 = Wallet::from_signer(signer1);
    let wallet2 = Wallet::from_signer(signer2);

    assert_eq!(wallet1.address, wallet2.address);
}
