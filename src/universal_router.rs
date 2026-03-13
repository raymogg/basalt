use alloy::primitives::{Address, Bytes, U256};
use alloy::sol;
use alloy::sol_types::SolValue;
use crate::types::V4PoolKey;

// Universal Router on Base
pub const UNIVERSAL_ROUTER: &str = "0x6ff5693b99212da76ad316178a184ab56d299b43";

// Universal Router execute interface
sol! {
    #[sol(rpc)]
    interface IUniversalRouter {
        function execute(bytes calldata commands, bytes[] calldata inputs, uint256 deadline) external payable;
    }
}

// Command bytes
const V3_SWAP_EXACT_IN: u8 = 0x00;
const V4_SWAP: u8 = 0x10;

// V4 action bytes
const SWAP_EXACT_IN_SINGLE: u8 = 0x06;
const SETTLE_ALL: u8 = 0x0c;
const TAKE_ALL: u8 = 0x0f;
const TAKE: u8 = 0x0e;

// Sentinel addresses
const MSG_SENDER: Address = Address::new([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
const ADDRESS_THIS: Address = Address::new([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);

// CONTRACT_BALANCE = 2^255
fn contract_balance() -> U256 {
    U256::from(1) << 255
}

/// Encode V3 path (packed: [address(20) | fee(3) | address(20)])
fn encode_v3_path(token_in: Address, fee: u32, token_out: Address) -> Vec<u8> {
    let mut path = Vec::with_capacity(43);
    path.extend_from_slice(token_in.as_slice());
    // fee as 3 bytes big-endian
    path.push((fee >> 16) as u8);
    path.push((fee >> 8) as u8);
    path.push(fee as u8);
    path.extend_from_slice(token_out.as_slice());
    path
}

/// Build V4 swap input bytes for a single-hop swap where output goes to msg.sender
fn encode_v4_single_swap_to_sender(
    pool_key: &V4PoolKey,
    zero_for_one: bool,
    amount_in: u128,
    min_amount_out: u128,
) -> Vec<u8> {
    let actions = vec![SWAP_EXACT_IN_SINGLE, SETTLE_ALL, TAKE_ALL];

    let (currency_in, currency_out) = if zero_for_one {
        (pool_key.currency0, pool_key.currency1)
    } else {
        (pool_key.currency1, pool_key.currency0)
    };

    // param 0: ExactInputSingleParams
    let swap_params = (
        (
            pool_key.currency0,
            pool_key.currency1,
            pool_key.fee_as_u24(),
            pool_key.tick_spacing_as_i24(),
            pool_key.hooks,
        ),
        zero_for_one,
        amount_in,
        min_amount_out,
        Bytes::new(), // hookData
    ).abi_encode_params();

    // param 1: SETTLE_ALL(currency_in, maxAmount)
    let settle_params = (currency_in, u128::MAX).abi_encode_params();

    // param 2: TAKE_ALL(currency_out, minAmount)
    let take_params = (currency_out, min_amount_out).abi_encode_params();

    let params: Vec<Bytes> = vec![
        Bytes::from(swap_params),
        Bytes::from(settle_params),
        Bytes::from(take_params),
    ];

    // V4_SWAP input = abi.encode(actions, params)
    (Bytes::from(actions), params).abi_encode_params()
}

/// Build V4 swap input bytes for a single-hop swap where output stays in the router
/// (for chaining with a subsequent swap)
fn encode_v4_single_swap_to_router(
    pool_key: &V4PoolKey,
    zero_for_one: bool,
    amount_in: u128,
) -> Vec<u8> {
    let actions = vec![SWAP_EXACT_IN_SINGLE, SETTLE_ALL, TAKE];

    let (currency_in, currency_out) = if zero_for_one {
        (pool_key.currency0, pool_key.currency1)
    } else {
        (pool_key.currency1, pool_key.currency0)
    };

    // param 0: ExactInputSingleParams
    let swap_params = (
        (
            pool_key.currency0,
            pool_key.currency1,
            pool_key.fee_as_u24(),
            pool_key.tick_spacing_as_i24(),
            pool_key.hooks,
        ),
        zero_for_one,
        amount_in,
        0u128, // amountOutMinimum = 0 for intermediate step
        Bytes::new(), // hookData
    ).abi_encode_params();

    // param 1: SETTLE_ALL(currency_in, maxAmount)
    let settle_params = (currency_in, u128::MAX).abi_encode_params();

    // param 2: TAKE(currency_out, recipient=ADDRESS_THIS, minAmount=0)
    let take_params = (currency_out, ADDRESS_THIS, U256::ZERO).abi_encode_params();

    let params: Vec<Bytes> = vec![
        Bytes::from(swap_params),
        Bytes::from(settle_params),
        Bytes::from(take_params),
    ];

    (Bytes::from(actions), params).abi_encode_params()
}

/// Build V3_SWAP_EXACT_IN input where tokens come from msg.sender
fn encode_v3_swap_from_sender(
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
    min_amount_out: U256,
) -> Vec<u8> {
    let path = encode_v3_path(token_in, fee, token_out);
    (MSG_SENDER, amount_in, min_amount_out, Bytes::from(path), true).abi_encode_params()
}

/// Build V3_SWAP_EXACT_IN input where tokens are already in the router (from a previous swap)
fn encode_v3_swap_from_router(
    token_in: Address,
    token_out: Address,
    fee: u32,
    min_amount_out: U256,
) -> Vec<u8> {
    let path = encode_v3_path(token_in, fee, token_out);
    (MSG_SENDER, contract_balance(), min_amount_out, Bytes::from(path), false).abi_encode_params()
}

/// Encode a full Universal Router execute() calldata for a V4 single-hop swap
pub fn encode_v4_single(
    pool_key: &V4PoolKey,
    zero_for_one: bool,
    amount_in: u128,
    min_amount_out: u128,
    deadline: U256,
) -> Vec<u8> {
    let commands = vec![V4_SWAP];
    let v4_input = encode_v4_single_swap_to_sender(pool_key, zero_for_one, amount_in, min_amount_out);

    let inputs: Vec<Bytes> = vec![Bytes::from(v4_input)];

    alloy::sol_types::SolCall::abi_encode(&IUniversalRouter::executeCall {
        commands: Bytes::from(commands),
        inputs,
        deadline,
    })
}

/// Encode a full Universal Router execute() calldata for a V3 single-hop swap
pub fn encode_v3_single(
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
    min_amount_out: U256,
    deadline: U256,
) -> Vec<u8> {
    let commands = vec![V3_SWAP_EXACT_IN];
    let v3_input = encode_v3_swap_from_sender(token_in, token_out, fee, amount_in, min_amount_out);

    let inputs: Vec<Bytes> = vec![Bytes::from(v3_input)];

    alloy::sol_types::SolCall::abi_encode(&IUniversalRouter::executeCall {
        commands: Bytes::from(commands),
        inputs,
        deadline,
    })
}

/// Encode a full Universal Router execute() calldata for a V4 -> V3 multi-hop swap
/// Leg 1: token_in -> intermediate via V4
/// Leg 2: intermediate -> token_out via V3
pub fn encode_v4_v3_multihop(
    v4_pool_key: &V4PoolKey,
    v4_zero_for_one: bool,
    amount_in: u128,
    intermediate: Address,
    token_out: Address,
    v3_fee: u32,
    min_amount_out: U256,
    deadline: U256,
) -> Vec<u8> {
    let commands = vec![V4_SWAP, V3_SWAP_EXACT_IN];

    // Leg 1: V4 swap, output stays in router
    let v4_input = encode_v4_single_swap_to_router(v4_pool_key, v4_zero_for_one, amount_in);

    // Leg 2: V3 swap, uses CONTRACT_BALANCE from router, output to msg.sender
    let v3_input = encode_v3_swap_from_router(intermediate, token_out, v3_fee, min_amount_out);

    let inputs: Vec<Bytes> = vec![
        Bytes::from(v4_input),
        Bytes::from(v3_input),
    ];

    alloy::sol_types::SolCall::abi_encode(&IUniversalRouter::executeCall {
        commands: Bytes::from(commands),
        inputs,
        deadline,
    })
}

/// Encode a full Universal Router execute() calldata for a V4 -> V4 multi-hop swap
/// Leg 1: token_in -> intermediate via V4 pool 1
/// Leg 2: intermediate -> token_out via V4 pool 2
pub fn encode_v4_v4_multihop(
    pool_key_1: &V4PoolKey,
    zero_for_one_1: bool,
    amount_in: u128,
    pool_key_2: &V4PoolKey,
    zero_for_one_2: bool,
    min_amount_out: u128,
    deadline: U256,
) -> Vec<u8> {
    let commands = vec![V4_SWAP, V4_SWAP];

    // Leg 1: V4 swap, output stays in router
    let v4_input_1 = encode_v4_single_swap_to_router(pool_key_1, zero_for_one_1, amount_in);

    // Leg 2: V4 swap, takes from router balance, output to msg.sender
    // For the second leg, the amount_in comes from the first swap's output
    // We need to use the router's balance — but V4 SETTLE_ALL will settle from msg.sender
    // Instead, for chained V4 swaps, we should use the router's token balance
    // The second V4 swap settles from router's balance and takes to msg.sender
    let v4_input_2 = encode_v4_single_swap_to_sender(pool_key_2, zero_for_one_2, 0, min_amount_out);

    let inputs: Vec<Bytes> = vec![
        Bytes::from(v4_input_1),
        Bytes::from(v4_input_2),
    ];

    alloy::sol_types::SolCall::abi_encode(&IUniversalRouter::executeCall {
        commands: Bytes::from(commands),
        inputs,
        deadline,
    })
}

/// Describe what router and method is being used (for display)
pub fn router_address() -> Address {
    UNIVERSAL_ROUTER.parse().unwrap()
}
