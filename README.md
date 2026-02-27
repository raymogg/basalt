# Basalt

**CLI toolkit for AI agents trading on Base.**

Basalt provides reliable on-chain swap quoting, trade execution, and portfolio tracking for tokens on Base. Built in Rust for performance and safety, designed for autonomous agent consumption.

## Design Principles

1. **On-chain first**: Use on-chain data where possible. Verify / supplement with off-chain data
2. **Efficiency**: Agents should not need hundreds of RPC calls. Cache as much data as possible.
3. **Fail loudly**: Errors are explicit.
4. **Agentic**: Clean CLI output, predictable exit codes, structured data

---

## Roadmap

- [x] Multi-DEX quote engine with caching
- [x] Bidirectional trading (any token pair)
- [ ] Trade execution with retry logic
- [ ] On-chain verification (balance changes)
- [ ] SQLite trade logging
- [ ] Portfolio tracking with P&L
- [ ] Balance reconciliation

---

## Installation & Build

```bash
# Clone the repository
git clone <repo-url>
cd basalt

# Build the binary
cargo build --release

# Binary will be at: target/release/basalt
# Or run directly with: cargo run --
```

---

## Usage

### Available Commands

**Quote:**
```bash
basalt quote <AMOUNT> --from <TOKEN_IN> --to <TOKEN_OUT>
```
Get real-time swap quotes for any token pair on Base.

**Refresh Cache:**
```bash
basalt refresh-cache [--test-amount <AMOUNT>]
```
Refresh all cached routes by re-quoting each token pair.

---

### Command: `quote`

Get real-time swap quotes for any token pair on Base.

**Syntax:**
```bash
basalt quote <AMOUNT> --from <TOKEN_IN> --to <TOKEN_OUT>
```

**Parameters:**
- `<AMOUNT>`: Amount of input token to swap (human-readable, e.g., "25" for 25 USDC)
- `--from <TOKEN_IN>`: Input token address or alias (`usdc`, `weth`, `eth`)
- `--to <TOKEN_OUT>`: Output token address or alias

**Examples:**

Buy a token with USDC:
```bash
basalt quote 25 --from usdc --to 0x1234567890abcdef1234567890abcdef12345678
```
Output:
```
Quoting: 25 USDC -> TOKEN

Routes:
  v3-direct(3000): 1000000 TOKEN
  v3-via-weth: 998500 TOKEN

Best:  1000000 TOKEN (v3-direct(3000))
Price: 1 USDC = 40000.000000 TOKEN
```

Sell a token for USDC:
```bash
basalt quote 1000000 --from 0x1234567890abcdef1234567890abcdef12345678 --to usdc
```
Output:
```
Quoting: 1000000 TOKEN -> USDC

Routes:
  v3-direct(3000): 24.52 USDC
  v4-weth-v3: 24.48 USDC

Best:  24.52 USDC (v3-direct(3000))
Price: 1 TOKEN = 0.000025 USDC
```

Swap between any two tokens:
```bash
basalt quote 0.01 --from weth --to 0x1234567890abcdef1234567890abcdef12345678
```

**Token Aliases:**

Instead of full addresses, you can use:
- `usdc` -> `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`
- `weth` or `eth` -> `0x4200000000000000000000000000000000000006`

**How It Works:**

1. Fetches token metadata (decimals, symbol) - cached to `~/.basalt/token-registry.json`
2. Queries multiple DEXs in parallel:
   - Uniswap V4 (with Clanker hooks discovery)
   - Uniswap V3 (multiple fee tiers)
   - Aerodrome (stable + volatile pools)
   - Multi-hop routes (via WETH)
3. Returns best route based on output amount
4. Caches successful routes to `~/.basalt/route-cache.json` for faster subsequent quotes

**Output Format:**

- `Routes`: All available swap paths with their output amounts
- `Best`: The route that gives you the most output tokens
- `Price`: Effective exchange rate (1 input token = X output tokens)

**For AI Agents:**

- Exit code 0 = successful quote found
- Exit code 1 = error (invalid address, no routes, RPC failure)
- Parse the "Best:" line to extract the optimal output amount
- Route method (e.g., `v3-direct(3000)`) indicates the DEX and fee tier used
- All amounts are human-readable (already divided by decimals)

---

### Command: `refresh-cache`

Refresh all cached routes by re-quoting each token pair. Useful for ensuring cached routes are still optimal.

**Syntax:**
```bash
basalt refresh-cache [--test-amount <AMOUNT>]
```

**Parameters:**
- `--test-amount <AMOUNT>`: Amount to use for test quotes (default: 1.0 of input token)

**Example:**
```bash
basalt refresh-cache
```
Output:
```
Refreshing all cached routes...
Found 2 token pairs in cache

[1/2] Refreshing USDC -> WETH... OK - 2 routes cached (best: v3-direct(500))
[2/2] Refreshing WETH -> USDC... OK - 2 routes cached (best: v3-direct(500))

============================================
Refreshed: 2
Failed:    0
Total:     2
============================================
```

**How It Works:**

1. Loads all token pairs from `~/.basalt/route-cache.json`
2. For each pair, performs a full route discovery (tries all DEXs and fee tiers)
3. Updates the cache with the best routes found
4. Resets usage counters

---

### Configuration

**Cache Refresh Interval:**

By default, cached routes are automatically refreshed every 10 quotes. Configure this with:
```bash
export BASALT_CACHE_REFRESH_INTERVAL=5
basalt quote 25 --from usdc --to weth
```

**Cache Location:**

All cache files are stored in `~/.basalt/`:
- `token-registry.json` - Token metadata (immutable: decimals, symbol, name)
- `route-cache.json` - Best routes per token pair with usage tracking

Routes are cached bidirectionally (A->B and B->A) since most pools support both directions.

---

## Supported DEXs on Base

- **Uniswap V4** (Clanker pools with dynamic fees)
- **Uniswap V3** (fee tiers: 500, 3000, 10000 bps)
- **Aerodrome** (stable and volatile pools)

Multi-hop routing automatically tried when direct routes fail.

---

## Examples for Agents

**Pre-buy check: How much TOKEN can I get for $25 USDC?**
```bash
basalt quote 25 --from usdc --to 0xTOKEN_ADDRESS
# Parse "Best: X TOKEN" line for the answer
```

**Pre-sell check: How much USDC will I get for my 1M TOKEN?**
```bash
basalt quote 1000000 --from 0xTOKEN_ADDRESS --to usdc
# Parse "Best: X USDC" line for realizable value
```

**Check if a pair has liquidity:**
```bash
basalt quote 1 --from 0xTOKEN_A --to 0xTOKEN_B
# Exit code 0 + routes found = liquid pair
# Exit code 1 or "No routes found" = illiquid
```

---
## License

MIT
