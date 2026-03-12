# Basalt Constants & Addresses

## Contract Addresses (Base Mainnet)

| Contract | Address |
|----------|---------|
| Universal Router | `0x6ff5693b99212da76ad316178a184ab56d299b43` |
| V4 Quoter | `0x0d5e0f971ed27fbff6c2837bf31316121532048d` |
| V4 StateView | `0xa3c0c9b65bad0b08107aa264b0f3db444b867a71` |
| V3 QuoterV2 | `0x3d4e44Eb1374240CE5F1B871ab261CD16335B76a` |
| Aerodrome Router | `0xcF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43` |
| Aerodrome Factory | `0x420DD381b31aEf6683db6B902084cB0FFECe40Da` |

## Token Addresses (Base Mainnet)

| Token | Address |
|-------|---------|
| USDC | `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913` |
| WETH | `0x4200000000000000000000000000000000000006` |
| flETH | `0x000000000D564D5be76f7f0d28fE52605afC7Cf8` |

## V3 Fee Tiers

| Fee | Basis Points |
|-----|-------------|
| 500 | 5 bps |
| 3000 | 30 bps |
| 10000 | 100 bps |

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `BASE_RPC_URL` | Yes | `https://base.rpc.blxrbdn.com` | Base RPC endpoint |
| `POOLSCOUT_API_KEY` | Yes | - | PoolScout API key for V4 pool discovery |
| `BASALT_CACHE_DIR` | No | `~/.basalt` | Cache directory |
| `BASALT_CACHE_REFRESH_INTERVAL` | No | `10` | Route cache refresh interval (number of uses) |

## Notes

- V4 pools require a full pool key (currency0, currency1, fee, tickSpacing, hooks) for quoting
- V4 pool IDs are `keccak256(abi.encode(poolKey))`
- Base chain has ~2 second block times
- Clanker is the main V4 liquidity source on Base (memecoin launcher)
- flETH is a popular intermediate token for V4 routes
