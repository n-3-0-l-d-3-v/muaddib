//! Processes and a wall-clock-free scheduler built on
//! `capability`'s `Kernel`. See `docs/design/KERNEL.md` and
//! `docs/design/decisions/ADR-002-processes-and-scheduling.md`.

mod process;
mod scheduler;
mod transfer;

pub use process::{Handle, Process, ProcessId};

/// A logical timestamp for an event at a process — the only notion of
/// "when" anything in this kernel has.
pub type Stamp = clock::Stamp<ProcessId>;
pub use scheduler::{ProcessError, Scheduler};
pub use transfer::{apply_grants, resolve_grants, Grant, GrantError};
