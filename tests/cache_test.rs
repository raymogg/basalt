use basalt::cache::*;
use basalt::types::QuoteResult;
use alloy::primitives::{Address, U256};
use std::sync::Mutex;

// Global mutex to ensure tests run serially (since they all use env vars)
static TEST_MUTEX: Mutex<()> = Mutex::new(());

fn mock_token_a() -> Address {
    "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913".parse().unwrap()
}

fn mock_token_b() -> Address {
    "0x4200000000000000000000000000000000000006".parse().unwrap()
}

/// Setup isolated test cache directory
fn setup_test_cache() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    std::env::set_var("BASALT_CACHE_DIR", temp_dir.path());
    temp_dir
}

fn mock_quotes() -> Vec<QuoteResult> {
    vec![
    QuoteResult {
        method: "v3-direct(500)".to_string(),
        amount_out: U256::from(1000000),
        gas_estimate: None,
        pool_id: None,
    },
    QuoteResult {
        method: "aerodrome-stable".to_string(),
        amount_out: U256::from(999500), // 0.05% worse
        gas_estimate: None,
        pool_id: None,
    },
    QuoteResult {
        method: "v3-direct(3000)".to_string(),
        amount_out: U256::from(999000), // 0.1% worse
        gas_estimate: None,
        pool_id: None,
    },
    QuoteResult {
        method: "v3-via-weth".to_string(),
        amount_out: U256::from(990000), // 1% worse - should be filtered out
        gas_estimate: None,
        pool_id: None,
    },
    ]
}

#[test]
fn test_cache_routes_stores_top_3_within_threshold() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache the routes
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Load and verify
    let cached = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();

    // Should have 3 routes (best + 2 within 0.5%)
    assert_eq!(cached.routes.len(), 3);

    // First route should be best
    assert_eq!(cached.routes[0].method, "v3-direct(500)");
    assert_eq!(cached.routes[0].relative_performance, 0.0);

    // Second route should be aerodrome
    assert_eq!(cached.routes[1].method, "aerodrome-stable");
    assert!(cached.routes[1].relative_performance < 0.005);

    // Third route should be v3-direct(3000)
    assert_eq!(cached.routes[2].method, "v3-direct(3000)");
    assert!(cached.routes[2].relative_performance < 0.005);

    // v3-via-weth should NOT be cached (>0.5% worse)
    assert!(!cached.routes.iter().any(|r| r.method == "v3-via-weth"));

    // Usage count should be 1 (we just called get_cached_routes)
    assert_eq!(cached.usage_count, 1);
    }

    #[test]
    fn test_bidirectional_caching() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache A -> B
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Both directions should be cached
    let forward = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();
    let reverse = get_cached_routes(token_b, token_a, 10).unwrap().unwrap();

    assert_eq!(forward.routes.len(), reverse.routes.len());
    assert_eq!(forward.routes[0].method, reverse.routes[0].method);
    }

    #[test]
    fn test_usage_counter_increments() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache routes
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Get cached routes multiple times
    let cached_1 = get_cached_routes(token_a, token_b, 10).unwrap();
    assert!(cached_1.is_some(), "Cache should exist after first call");
    assert_eq!(cached_1.unwrap().usage_count, 1);

    let cached_2 = get_cached_routes(token_a, token_b, 10).unwrap();
    assert!(cached_2.is_some(), "Cache should exist after second call");
    assert_eq!(cached_2.unwrap().usage_count, 2);

    let cached_3 = get_cached_routes(token_a, token_b, 10).unwrap();
    assert!(cached_3.is_some(), "Cache should exist after third call");
    assert_eq!(cached_3.unwrap().usage_count, 3);
    }

    #[test]
    fn test_cache_refresh_after_threshold() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache routes
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Use cache 9 times (refresh interval is 10)
    for i in 1..10 {
        let cached = get_cached_routes(token_a, token_b, 10).unwrap();
        assert!(cached.is_some(), "Should have cache on use {}", i);
    }

    // 10th use should trigger refresh (return None)
    let cached_10 = get_cached_routes(token_a, token_b, 10).unwrap();
    assert!(cached_10.is_none(), "Should return None to trigger refresh on 10th use");

    // After refresh trigger, usage count should reset to 0
    // Re-cache (simulating a full refresh)
    cache_routes(token_a, token_b, &quotes).unwrap();

    let cached_fresh = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();
    assert_eq!(cached_fresh.usage_count, 1);
    }

    #[test]
    fn test_empty_quotes_doesnt_cache() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();

    // Try to cache empty quotes
    cache_routes(token_a, token_b, &[]).unwrap();

    // Should not have created a cache entry
    let cached = get_cached_routes(token_a, token_b, 10).unwrap();
    assert!(cached.is_none());
    }

    #[test]
    fn test_cache_key_format() {
    let token_a = mock_token_a();
    let token_b = mock_token_b();

    let key = route_cache_key(token_a, token_b);

    // Should be "address_a:address_b"
    assert!(key.contains(':'));
    assert!(key.starts_with("0x"));
    assert_eq!(key.matches(':').count(), 1);
    }

    #[test]
    fn test_load_route_cache_returns_all_pairs() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache routes for token_a -> token_b
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Load the entire cache
    let cache = load_route_cache().unwrap();

    // Should have both forward and reverse entries
    assert!(cache.len() >= 2);

    let forward_key = route_cache_key(token_a, token_b);
    let reverse_key = route_cache_key(token_b, token_a);

    assert!(cache.contains_key(&forward_key));
    assert!(cache.contains_key(&reverse_key));
    }

    #[test]
    fn test_parse_cache_keys_into_token_pairs() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache routes
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Load cache and parse keys
    let cache = load_route_cache().unwrap();

    let pairs: Vec<(Address, Address)> = cache
        .keys()
        .filter_map(|key| {
            let parts: Vec<&str> = key.split(':').collect();
            if parts.len() == 2 {
                let token_in = parts[0].parse().ok()?;
                let token_out = parts[1].parse().ok()?;
                Some((token_in, token_out))
            } else {
                None
            }
        })
        .collect();

    // Should have 2 pairs (forward and reverse)
    assert_eq!(pairs.len(), 2);

    // Check that we can parse both directions
    assert!(pairs.contains(&(token_a, token_b)));
    assert!(pairs.contains(&(token_b, token_a)));
    }

    #[test]
    fn test_refresh_resets_usage_count() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Cache routes
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Use cache 5 times
    for _ in 0..5 {
        let _ = get_cached_routes(token_a, token_b, 10).unwrap();
    }

    // Verify usage count is 5
    let cached = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();
    assert_eq!(cached.usage_count, 6); // 5 + 1 from this call

    // Simulate a refresh by re-caching
    cache_routes(token_a, token_b, &quotes).unwrap();

    // Usage count should be reset to 0 (and then 1 after first get)
    let cached_fresh = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();
    assert_eq!(cached_fresh.usage_count, 1);
    }

    #[test]
    fn test_updated_at_changes_on_refresh() {
    let _guard = TEST_MUTEX.lock().unwrap();
    let _temp_dir = setup_test_cache();

    let token_a = mock_token_a();
    let token_b = mock_token_b();
    let quotes = mock_quotes();

    // Initial cache
    cache_routes(token_a, token_b, &quotes).unwrap();
    let initial = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();
    let initial_timestamp = initial.updated_at;

    // Wait a moment
    std::thread::sleep(std::time::Duration::from_secs(1));

    // Re-cache (simulating refresh)
    cache_routes(token_a, token_b, &quotes).unwrap();
    let refreshed = get_cached_routes(token_a, token_b, 10).unwrap().unwrap();

    // Timestamp should be newer
    assert!(refreshed.updated_at > initial_timestamp);
    }

    #[test]
    fn test_v3_fee_parsing_from_cached_method() {
    // Test that we can parse fee tiers from cached v3-direct methods
    let test_cases = vec![
        ("v3-direct(500)", Some(500u32)),
        ("v3-direct(3000)", Some(3000u32)),
        ("v3-direct(10000)", Some(10000u32)),
        ("v3-direct(invalid)", None),
        ("aerodrome-stable", None),
        ("v4-direct", None),
    ];

    for (method, expected_fee) in test_cases {
        let parsed_fee = if method.starts_with("v3-direct(") {
            method
                .strip_prefix("v3-direct(")
                .and_then(|s| s.strip_suffix(")"))
                .and_then(|s| s.parse::<u32>().ok())
        } else {
            None
        };

        assert_eq!(
            parsed_fee, expected_fee,
            "Failed to parse fee from method: {}",
            method
        );
    }
    }

    #[test]
    fn test_aerodrome_pool_type_parsing_from_cached_method() {
    // Test that we can determine pool type from cached aerodrome methods
    let test_cases = vec![
        ("aerodrome-stable", Some(true)),
        ("aerodrome-volatile", Some(false)),
        ("v3-direct(500)", None),
        ("v4-direct", None),
    ];

    for (method, expected_stable) in test_cases {
        let pool_type = if method.starts_with("aerodrome-") {
            Some(method == "aerodrome-stable")
        } else {
            None
        };

        assert_eq!(
            pool_type, expected_stable,
            "Failed to parse pool type from method: {}",
            method
        );
    }
    }

    #[test]
    fn test_v4_pool_params_parsing_from_cached_method() {
    use alloy::primitives::Address;

    // Test that we can parse V4 pool params from cached methods
    let test_cases = vec![
        (
            "v4-direct(8388608,200,0xbb7784a4d481184283ed89619a3e3ed143e1adc0)",
            Some((8388608u32, 200i32, "0xbb7784a4d481184283ed89619a3e3ed143e1adc0")),
        ),
        (
            "v4-direct(0,60,0xd60d6b218116cfd801e28f78d011a203d2b068cc)",
            Some((0u32, 60i32, "0xd60d6b218116cfd801e28f78d011a203d2b068cc")),
        ),
        ("v4-direct", None),
        ("v3-direct(500)", None),
        ("aerodrome-stable", None),
    ];

    for (method, expected) in test_cases {
        let parsed = if method.starts_with("v4-direct(") {
            method
                .strip_prefix("v4-direct(")
                .and_then(|s| s.strip_suffix(")"))
                .and_then(|params_str| {
                    let parts: Vec<&str> = params_str.split(',').collect();
                    if parts.len() == 3 {
                        let fee = parts[0].parse::<u32>().ok()?;
                        let tick_spacing = parts[1].parse::<i32>().ok()?;
                        let hooks = parts[2].parse::<Address>().ok()?;
                        Some((fee, tick_spacing, hooks))
                    } else {
                        None
                    }
                })
        } else {
            None
        };

        let expected_formatted = expected.map(|(f, t, h)| (f, t, h.parse::<Address>().unwrap()));

        assert_eq!(
            parsed, expected_formatted,
            "Failed to parse V4 params from method: {}",
            method
        );
    }
}
