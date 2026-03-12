# Basalt Usage

## Quoting

```bash
# Quote 100 tokens
cargo run --release -- quote 100 --from 0xTOKEN --to usdc

# Quote with symbol aliases
cargo run --release -- quote 1000 --from 0xTOKEN --to weth

# Quote stablecoins
cargo run --release -- quote 25 --from usdc --to weth
```

## Testing

```bash
# Run all tests
cargo test

# Run cache tests
cargo test --test cache_test

# Run universal router tests
cargo test --test universal_router_test
```

## Debugging

```bash
# Check cache contents
cat ~/.basalt/token-registry.json
cat ~/.basalt/route-cache.json
cat ~/.basalt/v4-pool-registry.json

# Clear cache (forces full rediscovery)
rm -rf ~/.basalt/

# Test with specific RPC
BASE_RPC_URL="https://base.llamarpc.com" cargo run --release -- quote 100 --from TOKEN --to usdc
```
