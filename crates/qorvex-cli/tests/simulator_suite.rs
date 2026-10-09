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
//!   cargo test -p qorvex-cli --test simulator_suite -- --ignored
//!
//! The tests share one simulator session, and concurrent device commands
//! contend and exceed the 15s command timeout (`<interrupted>`). The suite
//! therefore serializes itself: the first `harness()` call in a test takes a
//! process-wide lock held until that test ends, so default parallel test
//! threads are fine. Such timeouts are contention, not a product defect.
//!
//! All tests are #[ignore] by default so they don't run in `cargo test`.

mod simulator;
