//! The `randomsigned` trust decision, free of esp-hal so the host tests run it.
//!
//! A submit on a station that has no sign key creates one and sends `Add`; the key stays
//! *pending* (also in NVS, so across power cycles) until a handshake proves the server signs with
//! it. Until the admin accepts the request, the server keeps signing with the key it had before
//! (or with none), and the link may drop and come back any number of times meanwhile.

/// What the server's `randomsigned` header proved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature {
    /// No header: the server holds no key for this station (unknown to it, or not loaded yet).
    Missing,
    /// It decrypts with our key to the random we sent.
    Ours,
    /// It doesn't: the server signed with a key we don't have.
    Other,
}

/// What the station makes of the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// The server signed with our key. `confirms_pending`: this is the first proof that it
    /// accepted the key, so it stops being pending.
    Trusted { confirms_pending: bool },
    /// Not signed: not trusted, the key is kept.
    Unsigned,
    /// Our key is pending and the server still signs with another one: the request hasn't been
    /// accepted yet. Not trusted; the key is kept, so an approval that comes after a reconnect
    /// stores a key the station still has.
    AwaitingApproval,
    /// The station has no key ("Reset settings", or never added): it waits to be added again.
    NoKey,
    /// The server accepted our key once and now signs with another: a real trust failure.
    Untrusted,
}

pub fn decide(signature: Signature, sign_key: u32, pending: bool) -> Trust {
    match signature {
        Signature::Missing => Trust::Unsigned,
        // 0 means "no key": a server signing with the all-zero key proves nothing
        _ if sign_key == 0 => Trust::NoKey,
        Signature::Ours => Trust::Trusted {
            confirms_pending: pending,
        },
        Signature::Other if pending => Trust::AwaitingApproval,
        Signature::Other => Trust::Untrusted,
    }
}

/// The station has a key this connection's handshake didn't confirm while it is still pending:
/// it counts as not added (the screen asks for a submit, which re-sends the same key) and isn't
/// trusted on the strength of that submit.
pub fn awaiting_approval(pending: bool, trusted: bool) -> bool {
    pending && !trusted
}

/// Device settings pushed after the first ones of a connection mean the server's device list
/// changed, which is how an approval arrives. Trust is only decided in the handshake, so a
/// station still waiting for approval reconnects to check it again.
pub fn recheck_on_settings(pending: bool, trusted: bool, first_settings: bool) -> bool {
    awaiting_approval(pending, trusted) && !first_settings
}
