//! Real-simulator integration tests for qorvex CLI.
//!
//! These tests require:
//! - A booted iOS Simulator
//! - qorvex-testapp installed (`make -C qorvex-testapp run`)
//! - qorvex agent built (`make -C qorvex-agent build`)
//!
//! Run with:
//!   cargo test -p qorvex-cli --test simulator_suite -- --ignored --test-threads=1
//!
//! All tests are #[ignore] by default so they don't run in `cargo test`.

mod simulator;
