use crate::types::{CachedRoute, RouteCache, TokenMetadata, TokenRegistry, V4PoolRegistry, V4PoolKeyData};
use alloy::primitives::Address;
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_DIR: &str = ".basalt";

/// Get the cache directory path, creating it if it doesn't exist
/// Uses BASALT_CACHE_DIR env var if set (for testing), otherwise uses ~/.basalt
fn cache_dir() -> Result<PathBuf> {
    let path = if let Ok(test_dir) = std::env::var("BASALT_CACHE_DIR") {
        // Test mode: use provided directory
        PathBuf::from(test_dir)
    } else {
        // Production mode: use ~/.basalt
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(CACHE_DIR)
    };

    if !path.exists() {
        fs::create_dir_all(&path)?;
    }
    Ok(path)
}

/// Get current unix timestamp
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ============================================================================
// Token Registry Cache
// ============================================================================

/// Load token registry from JSON file
pub fn load_token_registry() -> Result<HashMap<Address, TokenRegistry>> {
    let path = cache_dir()?.join("token-registry.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let contents = fs::read_to_string(path)?;
    let json: HashMap<String, TokenRegistry> = serde_json::from_str(&contents)?;

    // Convert string keys to Address
    let mut registry = HashMap::new();
    for (addr_str, meta) in json {
        if let Ok(addr) = addr_str.parse::<Address>() {
            registry.insert(addr, meta);
        }
    }

    Ok(registry)
}

/// Save token registry to JSON file
pub fn save_token_registry(registry: &HashMap<Address, TokenRegistry>) -> Result<()> {
    let path = cache_dir()?.join("token-registry.json");

    // Convert Address keys to strings for JSON serialization
    let json: HashMap<String, TokenRegistry> = registry
        .iter()
        .map(|(addr, meta)| (addr.to_string(), meta.clone()))
        .collect();

    let contents = serde_json::to_string_pretty(&json)?;
    fs::write(path, contents)?;

    Ok(())
}

/// Get token metadata from cache
pub fn get_cached_token(address: Address) -> Result<Option<TokenRegistry>> {
    let registry = load_token_registry()?;
    Ok(registry.get(&address).cloned())
}

/// Cache token metadata
pub fn cache_token(metadata: &TokenMetadata) -> Result<()> {
    let mut registry = load_token_registry()?;

    registry.insert(
        metadata.address,
        TokenRegistry {
            decimals: metadata.decimals,
            symbol: metadata.symbol.clone(),
            name: metadata.name.clone(),
            cached_at: now(),
        },
    );

    save_token_registry(&registry)?;
    Ok(())
}

// ============================================================================
// Route Cache
// ============================================================================

/// Load route cache from JSON file
/// Cache keys are "token_in:token_out" pairs
pub fn load_route_cache() -> Result<HashMap<String, RouteCache>> {
    let path = cache_dir()?.join("route-cache.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }

    let contents = fs::read_to_string(path)?;
    let cache: HashMap<String, RouteCache> = serde_json::from_str(&contents)?;
    Ok(cache)
}

/// Save route cache to JSON file
pub fn save_route_cache(cache: &HashMap<String, RouteCache>) -> Result<()> {
    let path = cache_dir()?.join("route-cache.json");
    let contents = serde_json::to_string_pretty(&cache)?;
    fs::write(path, contents)?;
    Ok(())
}

/// Generate cache key for a token pair
pub fn route_cache_key(token_in: Address, token_out: Address) -> String {
    format!("{}:{}", token_in, token_out)
}

/// Get cached routes for a token pair
/// Returns None if cache doesn't exist or needs refresh
pub fn get_cached_routes(
    token_in: Address,
    token_out: Address,
    refresh_interval: u64, // Refresh cache every N uses
) -> Result<Option<RouteCache>> {
    let mut cache = load_route_cache()?;
    let key = route_cache_key(token_in, token_out);

    if let Some(mut route_cache) = cache.get(&key).cloned() {
        route_cache.usage_count += 1;

        // Check if we need to refresh (every N quotes)
        if route_cache.usage_count >= refresh_interval {
            // Reset usage count and return None to trigger full refresh
            route_cache.usage_count = 0;
            cache.insert(key, route_cache);
            save_route_cache(&cache)?;
            return Ok(None);
        }

        // Update usage count
        cache.insert(key, route_cache.clone());
        save_route_cache(&cache)?;

        return Ok(Some(route_cache));
    }

    Ok(None)
}

/// Cache top routes for a token pair (within 0.5% of best, max 3)
/// Also caches the reverse direction with the same routes
pub fn cache_routes(
    token_in: Address,
    token_out: Address,
    quotes: &[crate::types::QuoteResult],
) -> Result<()> {
    if quotes.is_empty() {
        return Ok(());
    }

    let mut cache = load_route_cache()?;

    // Find best quote
    let best_amount = quotes.iter().map(|q| q.amount_out).max().unwrap();
    let best_amount_f64 = best_amount.to_string().parse::<f64>().unwrap_or(0.0);

    if best_amount_f64 == 0.0 {
        return Ok(());
    }

    // Filter routes within 0.5% of best
    const MAX_DEVIATION: f64 = 0.005; // 0.5%
    let mut viable_routes: Vec<CachedRoute> = quotes
        .iter()
        .filter_map(|q| {
            let amount_f64 = q.amount_out.to_string().parse::<f64>().unwrap_or(0.0);
            let deviation = (best_amount_f64 - amount_f64) / best_amount_f64;

            if deviation <= MAX_DEVIATION {
                Some(CachedRoute {
                    method: q.method.clone(),
                    relative_performance: deviation,
                })
            } else {
                None
            }
        })
        .collect();

    // Sort by performance (best first) and take top 3
    viable_routes.sort_by(|a, b| {
        a.relative_performance
            .partial_cmp(&b.relative_performance)
            .unwrap()
    });
    viable_routes.truncate(3);

    if viable_routes.is_empty() {
        return Ok(());
    }

    let route_cache = RouteCache {
        routes: viable_routes.clone(),
        updated_at: now(),
        usage_count: 0,
    };

    // Cache forward direction
    let forward_key = route_cache_key(token_in, token_out);
    cache.insert(forward_key, route_cache.clone());

    // Cache reverse direction (same routes work both ways for most pools)
    let reverse_key = route_cache_key(token_out, token_in);
    cache.insert(reverse_key, route_cache);

    save_route_cache(&cache)?;
    Ok(())
}

// ============================================================================
// V4 Pool Key Cache (in-memory for now, could persist later)
// ============================================================================

use std::sync::Mutex;
use std::sync::OnceLock;

static POOL_KEY_CACHE: OnceLock<Mutex<HashMap<String, crate::types::V4PoolInfo>>> = OnceLock::new();

/// Get pool key cache
fn pool_key_cache() -> &'static Mutex<HashMap<String, crate::types::V4PoolInfo>> {
    POOL_KEY_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Generate cache key for V4 pool lookup
pub fn pool_cache_key(token: Address, quote_token: Address) -> String {
    format!("{}_{}", token, quote_token)
}

/// Get cached V4 pool info
pub fn get_cached_pool(key: &str) -> Option<crate::types::V4PoolInfo> {
    let cache = pool_key_cache().lock().unwrap();
    cache.get(key).cloned()
}

/// Cache V4 pool info
pub fn cache_pool(key: String, pool_info: crate::types::V4PoolInfo) {
    let mut cache = pool_key_cache().lock().unwrap();
    cache.insert(key.clone(), pool_info.clone());

    // Also add to persistent global registry
    let _ = add_to_v4_pool_registry(&pool_info.pool_key);
}

// ============================================================================
// V4 Pool Registry (Persistent Global Pool Cache)
// ============================================================================

/// Load V4 pool registry from disk
pub fn load_v4_pool_registry() -> Result<V4PoolRegistry> {
    let path = cache_dir()?.join("v4-pool-registry.json");
    if !path.exists() {
        return Ok(V4PoolRegistry {
            pools: HashMap::new(),
            updated_at: now(),
        });
    }

    let contents = fs::read_to_string(path)?;
    let registry: V4PoolRegistry = serde_json::from_str(&contents)?;
    Ok(registry)
}

/// Save V4 pool registry to disk
fn save_v4_pool_registry(registry: &V4PoolRegistry) -> Result<()> {
    let path = cache_dir()?.join("v4-pool-registry.json");
    let json = serde_json::to_string_pretty(registry)?;
    fs::write(path, json)?;
    Ok(())
}

/// Add a pool to the V4 registry
pub fn add_to_v4_pool_registry(pool_key: &crate::types::V4PoolKey) -> Result<()> {
    let mut registry = load_v4_pool_registry()?;

    // Create sorted key for lookup
    let (addr0, addr1) = if pool_key.currency0 < pool_key.currency1 {
        (pool_key.currency0, pool_key.currency1)
    } else {
        (pool_key.currency1, pool_key.currency0)
    };
    let key = format!("{}_{}_{}_{}_{}",
        addr0.to_string().to_lowercase(),
        addr1.to_string().to_lowercase(),
        pool_key.fee,
        pool_key.tick_spacing,
        pool_key.hooks.to_string().to_lowercase()
    );

    // Add if not already present
    if !registry.pools.contains_key(&key) {
        registry.pools.insert(key, V4PoolKeyData {
            currency0: pool_key.currency0.to_string(),
            currency1: pool_key.currency1.to_string(),
            fee: pool_key.fee,
            tick_spacing: pool_key.tick_spacing,
            hooks: pool_key.hooks.to_string(),
            discovered_at: now(),
        });
        registry.updated_at = now();
        save_v4_pool_registry(&registry)?;
    }

    Ok(())
}

/// Get all known V4 pools for a token pair
pub fn get_v4_pools_for_pair(token_a: Address, token_b: Address) -> Result<Vec<crate::types::V4PoolKey>> {
    let registry = load_v4_pool_registry()?;
    let mut pools = Vec::new();

    // Sort addresses for lookup
    let (addr0, addr1) = if token_a < token_b {
        (token_a, token_b)
    } else {
        (token_b, token_a)
    };

    let prefix = format!("{}_{}_",
        addr0.to_string().to_lowercase(),
        addr1.to_string().to_lowercase()
    );

    // Find all pools matching this pair
    for (key, pool_data) in &registry.pools {
        if key.starts_with(&prefix) {
            if let (Ok(c0), Ok(c1), Ok(hooks)) = (
                pool_data.currency0.parse(),
                pool_data.currency1.parse(),
                pool_data.hooks.parse(),
            ) {
                pools.push(crate::types::V4PoolKey {
                    currency0: c0,
                    currency1: c1,
                    fee: pool_data.fee,
                    tick_spacing: pool_data.tick_spacing,
                    hooks,
                });
            }
        }
    }

    Ok(pools)
}
