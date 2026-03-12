# Basalt Architecture

## Core Components

### DEX Modules (`src/dex/`)
- `v4.rs` - Uniswap V4 pool discovery and quoting
- `v3.rs` - Uniswap V3 quoting (multi-fee: 500, 3000, 10000)
- `aerodrome.rs` - Aerodrome (V2-style) quoting
- `token.rs` - ERC20 token metadata fetching

### Quote Engine (`src/quote.rs`)
- Multi-route discovery (V4 direct, V4->V4, V4->V3, V3, Aerodrome)
- All routes execute in parallel via `tokio::spawn`
- Route caching (stores top 3 routes within 0.5% of best)
- Automatic route refresh based on usage count
- Falls back to full discovery if all cached routes fail

### PoolScout Integration (`src/poolscout.rs`)
- Single API call per quote (`get_pools_by_token`)
- Replaces old brute-force V4 pool discovery
- All pools cached locally after fetch
- No redundant API calls — routing derived from cached list

### Universal Router (`src/universal_router.rs`)
- Encodes Uniswap Universal Router `execute()` calldata
- Supports V4 single-hop, V3 single-hop, V4->V3 multi-hop, V4->V4 multi-hop
- All swaps encoded as atomic transactions
- 0.5% slippage tolerance, 5-minute deadline
- Router address on Base: `0x6ff5693b99212da76ad316178a184ab56d299b43`

### Caching System (`src/cache.rs`)
- Token metadata cache (`~/.basalt/token-registry.json`)
- Route cache (`~/.basalt/route-cache.json`)
- V4 pool registry (`~/.basalt/v4-pool-registry.json`)
- In-memory V4 pool key cache (session-scoped)

### Discovery Flow
```
Quote request
    |
    v
Route cache hit? --> Yes --> try cached routes --> any succeed? --> return quotes
    |                                                  |
    No                                                No (fall back)
    |                                                  |
    v                                                  v
In-memory pool cache --> Persistent registry --> PoolScout API (1 call)
    |                         |                        |
    v                         v                        v
               Cache all pools locally
                        |
                        v
              Try all routes in parallel
              (V4 direct, V4 multi-hop, V3, Aerodrome)
                        |
                        v
              Generate Universal Router calldata for best route
```

### Multi-hop Intermediates
Only WETH, USDC, and flETH are used as intermediate tokens for multi-hop routes.

## Key Files

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI entry point, argument parsing |
| `src/quote.rs` | Main quoting logic, route discovery |
| `src/dex/v4.rs` | V4 pool discovery and quoting |
| `src/dex/v3.rs` | V3 multi-fee quoting |
| `src/dex/aerodrome.rs` | Aerodrome integration |
| `src/dex/token.rs` | Token metadata fetching |
| `src/poolscout.rs` | PoolScout API client |
| `src/universal_router.rs` | Universal Router calldata encoding |
| `src/cache.rs` | All caching logic |
| `src/constants.rs` | Contract addresses, tokens |
| `src/types.rs` | Shared types (PoolKey, QuoteResult, etc.) |
| `src/dexscreener.rs` | DexScreener API (legacy, replaced by PoolScout) |
