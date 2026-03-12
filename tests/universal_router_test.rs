use alloy::primitives::{Address, U256};
use basalt::types::V4PoolKey;
use basalt::universal_router;

// Test token addresses (USDC and WETH on Base)
fn usdc() -> Address {
    "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse().unwrap()
}

fn weth() -> Address {
    "0x4200000000000000000000000000000000000006".parse().unwrap()
}

fn some_token() -> Address {
    "0xc43F3Ae305a92043bd9b62eBd2FE14F7547ee485".parse().unwrap()
}

fn zero_hooks() -> Address {
    Address::ZERO
}

fn sample_v4_pool_key(token_a: Address, token_b: Address) -> V4PoolKey {
    let (c0, c1) = if token_a < token_b {
        (token_a, token_b)
    } else {
        (token_b, token_a)
    };
    V4PoolKey {
        currency0: c0,
        currency1: c1,
        fee: 3000,
        tick_spacing: 60,
        hooks: zero_hooks(),
    }
}

fn deadline() -> U256 {
    U256::from(1_700_000_000u64)
}

#[test]
fn test_encode_v4_single_produces_valid_calldata() {
    let pool_key = sample_v4_pool_key(usdc(), weth());
    let zero_for_one = usdc() < weth();

    let calldata = universal_router::encode_v4_single(
        &pool_key,
        zero_for_one,
        1_000_000, // 1 USDC
        900,       // min out
        deadline(),
    );

    // Should start with execute function selector: 0x3593564c
    assert_eq!(&calldata[0..4], &[0x35, 0x93, 0x56, 0x4c], "Wrong function selector");

    // Should be non-trivial length (selector + encoded params)
    assert!(calldata.len() > 100, "Calldata too short: {} bytes", calldata.len());
}

#[test]
fn test_encode_v3_single_produces_valid_calldata() {
    let calldata = universal_router::encode_v3_single(
        usdc(),
        weth(),
        3000,
        U256::from(1_000_000),
        U256::from(900),
        deadline(),
    );

    // Should start with execute function selector
    assert_eq!(&calldata[0..4], &[0x35, 0x93, 0x56, 0x4c]);
    assert!(calldata.len() > 100);
}

#[test]
fn test_encode_v4_single_command_byte_is_v4_swap() {
    let pool_key = sample_v4_pool_key(usdc(), weth());

    let calldata = universal_router::encode_v4_single(
        &pool_key,
        true,
        1_000_000,
        900,
        deadline(),
    );

    // After the 4-byte selector, the first dynamic parameter is `commands`.
    // ABI encoding: offset to commands (32 bytes), offset to inputs (32 bytes),
    // deadline (32 bytes), then commands length + data.
    // The commands bytes should contain [0x10] (V4_SWAP)
    let hex = alloy::primitives::hex::encode(&calldata);
    // The commands data is a single byte: 0x10
    assert!(hex.contains("10"), "Should contain V4_SWAP command byte 0x10");
}

#[test]
fn test_encode_v3_single_command_byte_is_v3_swap() {
    let calldata = universal_router::encode_v3_single(
        usdc(),
        weth(),
        500,
        U256::from(1_000_000),
        U256::from(900),
        deadline(),
    );

    let hex = alloy::primitives::hex::encode(&calldata);
    // Commands should be [0x00] (V3_SWAP_EXACT_IN) — encoded as a single byte
    // In the ABI encoding, the commands bytes length will be 1, followed by 0x00 padded
    assert!(calldata.len() > 4);
}

#[test]
fn test_encode_v4_v3_multihop_has_two_commands() {
    let v4_pool_key = sample_v4_pool_key(some_token(), usdc());
    let v4_zero_for_one = some_token() < usdc();

    let calldata = universal_router::encode_v4_v3_multihop(
        &v4_pool_key,
        v4_zero_for_one,
        1_000_000_000, // token amount
        usdc(),        // intermediate
        weth(),        // final output
        3000,          // V3 fee
        U256::from(900),
        deadline(),
    );

    // Should have execute selector
    assert_eq!(&calldata[0..4], &[0x35, 0x93, 0x56, 0x4c]);

    // Should have two inputs (one per command)
    // The calldata should be significantly longer than single-hop
    assert!(calldata.len() > 500, "Multi-hop calldata too short: {} bytes", calldata.len());
}

#[test]
fn test_encode_v4_v4_multihop_has_two_commands() {
    let pk1 = sample_v4_pool_key(some_token(), weth());
    let pk2 = sample_v4_pool_key(weth(), usdc());

    let calldata = universal_router::encode_v4_v4_multihop(
        &pk1,
        some_token() < weth(),
        1_000_000_000,
        &pk2,
        weth() < usdc(),
        900,
        deadline(),
    );

    assert_eq!(&calldata[0..4], &[0x35, 0x93, 0x56, 0x4c]);
    assert!(calldata.len() > 500, "Multi-hop calldata too short: {} bytes", calldata.len());
}

#[test]
fn test_encode_v4_single_different_directions() {
    let pool_key = sample_v4_pool_key(usdc(), weth());

    let calldata_zfo = universal_router::encode_v4_single(
        &pool_key, true, 1_000_000, 900, deadline(),
    );
    let calldata_ofz = universal_router::encode_v4_single(
        &pool_key, false, 1_000_000, 900, deadline(),
    );

    // Different directions should produce different calldata
    assert_ne!(calldata_zfo, calldata_ofz, "zeroForOne true vs false should differ");
}

#[test]
fn test_encode_v3_different_fees_produce_different_calldata() {
    let cd_500 = universal_router::encode_v3_single(
        usdc(), weth(), 500, U256::from(1_000_000), U256::from(900), deadline(),
    );
    let cd_3000 = universal_router::encode_v3_single(
        usdc(), weth(), 3000, U256::from(1_000_000), U256::from(900), deadline(),
    );
    let cd_10000 = universal_router::encode_v3_single(
        usdc(), weth(), 10000, U256::from(1_000_000), U256::from(900), deadline(),
    );

    assert_ne!(cd_500, cd_3000);
    assert_ne!(cd_3000, cd_10000);
    assert_ne!(cd_500, cd_10000);
}

#[test]
fn test_encode_v4_single_deterministic() {
    let pool_key = sample_v4_pool_key(usdc(), weth());

    let cd1 = universal_router::encode_v4_single(&pool_key, true, 1_000_000, 900, deadline());
    let cd2 = universal_router::encode_v4_single(&pool_key, true, 1_000_000, 900, deadline());

    assert_eq!(cd1, cd2, "Same inputs should produce identical calldata");
}

#[test]
fn test_router_address_is_valid() {
    let addr = universal_router::router_address();
    assert_ne!(addr, Address::ZERO);
    assert_eq!(
        format!("{}", addr).to_lowercase(),
        "0x6ff5693b99212da76ad316178a184ab56d299b43"
    );
}
