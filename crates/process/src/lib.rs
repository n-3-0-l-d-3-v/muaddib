//! Processes and a wall-clock-free scheduler built on
//! `capability`'s `Kernel`. See `docs/design/KERNEL.md` and
//! `docs/design/decisions/ADR-002-processes-and-scheduling.md`.

mod process;
mod scheduler;
mod transfer;

pub use process::{Handle, Process, ProcessId};
pub use scheduler::{ProcessError, Scheduler};
pub use transfer::{apply_grants, resolve_grants, Grant, GrantError};
