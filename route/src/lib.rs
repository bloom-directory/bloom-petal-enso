pub mod api;
pub mod api_types;
pub mod input;
pub mod policy;
pub mod redaction;
pub mod runtime;
pub mod session;
pub mod settings;
pub mod settlement;
pub mod simulation;
pub mod wallet;
pub mod workflow;

#[cfg(test)]
mod integration_tests;

pub mod prelude {
    pub use crate::workflow::*;
    pub use petal::*;
}

/// Trusted account selected by Bloom; legacy hosts omit it and select account 0.
pub fn account_number(ctx: &petal::Ctx) -> u32 {
    petal::route_param(ctx, "bloom.account")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
