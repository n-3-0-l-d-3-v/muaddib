use std::fmt;
use std::ops::{BitAnd, BitOr};

/// What a capability permits — a small hand-rolled bitflag set (no
/// external `bitflags` dependency needed for five bits). `GRANT`
/// controls delegation (`Kernel::derive`); `DESTROY` controls
/// `Kernel::destroy_object`. Neither `READ`/`WRITE`/`EXECUTE` carries
/// any built-in semantics here — this crate is about the security
/// discipline (unforgeability, revocation, attenuation), not about what
/// a "read" of a not-yet-defined resource type means; later tickets
/// (memory regions, IPC channels) give these bits real meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rights(u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EXECUTE: Rights = Rights(1 << 2);
    pub const GRANT: Rights = Rights(1 << 3);
    pub const DESTROY: Rights = Rights(1 << 4);
    pub const ALL: Rights =
        Rights(Self::READ.0 | Self::WRITE.0 | Self::EXECUTE.0 | Self::GRANT.0 | Self::DESTROY.0);

    /// True if `self` includes every bit set in `required` — the check
    /// every capability validation ultimately boils down to.
    pub fn contains(self, required: Rights) -> bool {
        self.0 & required.0 == required.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Rights {
    type Output = Rights;
    fn bitor(self, rhs: Rights) -> Rights {
        Rights(self.0 | rhs.0)
    }
}

impl BitAnd for Rights {
    type Output = Rights;
    fn bitand(self, rhs: Rights) -> Rights {
        Rights(self.0 & rhs.0)
    }
}

impl fmt::Display for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = [
            (Rights::READ, "READ"),
            (Rights::WRITE, "WRITE"),
            (Rights::EXECUTE, "EXECUTE"),
            (Rights::GRANT, "GRANT"),
            (Rights::DESTROY, "DESTROY"),
        ];
        let held: Vec<&str> = names
            .iter()
            .filter(|(r, _)| self.contains(*r))
            .map(|(_, n)| *n)
            .collect();
        if held.is_empty() {
            write!(f, "NONE")
        } else {
            write!(f, "{}", held.join("|"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_checks_every_required_bit() {
        let rw = Rights::READ | Rights::WRITE;
        assert!(rw.contains(Rights::READ));
        assert!(rw.contains(Rights::WRITE));
        assert!(rw.contains(Rights::READ | Rights::WRITE));
        assert!(!rw.contains(Rights::EXECUTE));
        assert!(!rw.contains(Rights::READ | Rights::EXECUTE));
    }

    #[test]
    fn none_contains_nothing_but_none() {
        assert!(Rights::NONE.contains(Rights::NONE));
        assert!(!Rights::NONE.contains(Rights::READ));
    }

    #[test]
    fn all_contains_everything() {
        for r in [
            Rights::READ,
            Rights::WRITE,
            Rights::EXECUTE,
            Rights::GRANT,
            Rights::DESTROY,
        ] {
            assert!(Rights::ALL.contains(r));
        }
    }

    #[test]
    fn display_lists_held_rights() {
        assert_eq!((Rights::READ | Rights::GRANT).to_string(), "READ|GRANT");
        assert_eq!(Rights::NONE.to_string(), "NONE");
    }
}
