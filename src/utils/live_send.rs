//! When the logger task sends a batch (no deps, host-tested).

/// Live state while a solve is active, and log lines whenever there are some.
pub const SEND_INTERVAL_MS: u64 = 1000;
/// Header-only keep-alive of an idle station (the live page marks it offline after 30 s).
pub const IDLE_SEND_INTERVAL_MS: u64 = 5000;
/// A start/stop or competitor change goes out right away, but no sooner than this after the
/// last batch.
pub const MIN_GAP_MS: u64 = 100;

/// Whether to send a batch now.
///
/// `active`: a competitor is scanned in, or inspection or the solve timer is running.
/// `has_logs`: log lines are waiting. `since_last_ms`: time since the last send attempt.
/// `changed`: the running flags or the competitor differ from the last batch sent.
pub fn should_send(active: bool, has_logs: bool, since_last_ms: u64, changed: bool) -> bool {
    if since_last_ms < MIN_GAP_MS {
        return false;
    }
    let interval = if active || has_logs {
        SEND_INTERVAL_MS
    } else {
        IDLE_SEND_INTERVAL_MS
    };
    changed || since_last_ms >= interval
}
