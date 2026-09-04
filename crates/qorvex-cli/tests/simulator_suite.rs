//! Real-simulator integration tests for qorvex CLI.
//!
//! The suite owns its simulator: it creates and boots one named `qorvex-test`
//! (override with `QORVEX_TEST_SIM`, a name or UDID), installs the testapp on
//! it, and pins every command to that UDID — it never targets `booted`, so
//! other simulators may stay up. At exit it stops the session and shuts the
//! simulator down again, unless it was already booted before the run.
//!
//! These tests require:
//! - qorvex-testapp built (`make -C qorvex-testapp build`)
//! - qorvex agent built (`make -C qorvex-agent build`)
//!
//! Run with:
//!   cargo test -p qorvex-cli --test simulator_suite -- --ignored --test-threads=1
//!
//! All tests are #[ignore] by default so they don't run in `cargo test`.

mod simulator;
