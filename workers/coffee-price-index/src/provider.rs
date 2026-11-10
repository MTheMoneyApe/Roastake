//! Provider abstraction for coffee price index data sources.
//!
//! In Mission 01, only the deterministic `FixtureProvider` is active.
//! Future providers (ICO, ICE, Verified Suppliers) are defined as extension points
//! and fail-closed until external ingestion terms, licenses, and adapters are verified.

use roastake_shared::{
    WorkerOutput, DATA_CLASS_SYNTHETIC, DECIMALS_COFFEE_INDEX, DEFAULT_FIXTURE_PRICE_STR,
    MARKET_GESHA_DEMO, SCHEMA_VERSION, UNIT_USD_PER_KG,
};

/// Common trait for all coffee price data providers.
pub trait PriceProvider {
    /// Return the provider identifier name.
    fn name(&self) -> &'static str;

    /// Retrieve or compute a price observation for the specified market.
    fn fetch_price(&self, market: &str) -> Result<WorkerOutput, String>;
}

/// Deterministic synthetic demonstration fixture provider.
pub struct FixtureProvider;

impl PriceProvider for FixtureProvider {
    fn name(&self) -> &'static str {
        "roastake-fixture"
    }

    fn fetch_price(&self, market: &str) -> Result<WorkerOutput, String> {
        if market != MARKET_GESHA_DEMO {
            return Err(format!(
                "FixtureProvider only supports market '{}', requested '{}'",
                MARKET_GESHA_DEMO, market
            ));
        }

        Ok(WorkerOutput {
            schema_version: SCHEMA_VERSION,
            market: MARKET_GESHA_DEMO.to_string(),
            price: DEFAULT_FIXTURE_PRICE_STR.to_string(),
            decimals: DECIMALS_COFFEE_INDEX,
            unit: UNIT_USD_PER_KG.to_string(),
            source: self.name().to_string(),
            source_count: 1,
            observed_at: 0,
            data_class: DATA_CLASS_SYNTHETIC.to_string(),
        })
    }
}

/// Extension point: International Coffee Organization (ICO) composite indicator.
/// NOT implemented in Phase 1 (fails closed).
#[allow(dead_code)]
pub struct IcoProvider;

impl PriceProvider for IcoProvider {
    fn name(&self) -> &'static str {
        "ico-composite"
    }

    fn fetch_price(&self, _market: &str) -> Result<WorkerOutput, String> {
        Err(
            "ICO data feed is an unverified external source; access terms, network allowlist, \
             and normalization methodology must be verified before enabling on Testnet."
                .to_string(),
        )
    }
}

/// Extension point: Intercontinental Exchange (ICE) Coffee 'C' futures benchmark.
/// NOT implemented in Phase 1 (fails closed).
#[allow(dead_code)]
pub struct IceProvider;

impl PriceProvider for IceProvider {
    fn name(&self) -> &'static str {
        "ice-coffee-c"
    }

    fn fetch_price(&self, _market: &str) -> Result<WorkerOutput, String> {
        Err(
            "ICE Coffee 'C' futures feed is an unverified external source; commercial license, \
             contract month rolls, and delivery differentials must be established first."
                .to_string(),
        )
    }
}

/// Extension point: Verified physical supplier direct trade quotations.
/// NOT implemented in Phase 1 (fails closed).
#[allow(dead_code)]
pub struct VerifiedSupplierProvider;

impl PriceProvider for VerifiedSupplierProvider {
    fn name(&self) -> &'static str {
        "verified-supplier"
    }

    fn fetch_price(&self, _market: &str) -> Result<WorkerOutput, String> {
        Err(
            "Verified supplier ingestion requires on-chain supplier registration, \
             cryptographic warehouse receipts, and physical quality audit verification."
                .to_string(),
        )
    }
}
