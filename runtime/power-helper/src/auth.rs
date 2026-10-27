//! Who is on the other end of the socket: the kernel's audit token for the
//! peer, checked against a code requirement by the Security framework.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use core_foundation::base::TCFType;
use core_foundation::data::CFData;
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};

/// True when the peer's running code satisfies `requirement`. Anything
/// that cannot be read or checked is a refusal.
pub fn peer_is_host(stream: &UnixStream, requirement: &str) -> bool {
    // `audit_token_t` is eight 32-bit words; libc does not define it.
    let mut token = [0u32; 8];
    let mut len = std::mem::size_of_val(&token) as libc::socklen_t;
    // SAFETY: `token` is a writable buffer of `len` bytes, and the kernel
    // writes at most `len` bytes into it.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of_val(&token) {
        return false;
    }
    let bytes: Vec<u8> = token.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let data = CFData::from_buffer(&bytes);
    let mut attrs = GuestAttributes::new();
    attrs.set_audit_token(data.as_concrete_TypeRef());
    let Ok(code) = SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE) else {
        return false;
    };
    let Ok(requirement) = requirement.parse::<SecRequirement>() else {
        return false;
    };
    code.check_validity(Flags::NONE, &requirement).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test binary is not the signed Zwiim Host.
    #[test]
    fn an_unsigned_peer_is_refused() {
        let (a, _b) = UnixStream::pair().unwrap();
        let requirement = crate::core::caller_requirement("ABCDE12345").unwrap();
        assert!(!peer_is_host(&a, &requirement));
    }

    /// The check really runs: a requirement any code meets is accepted.
    #[test]
    fn a_satisfied_requirement_is_accepted() {
        let (a, _b) = UnixStream::pair().unwrap();
        assert!(peer_is_host(&a, "always"));
        assert!(!peer_is_host(&a, "never"));
        assert!(!peer_is_host(&a, "not a requirement"));
    }
}
