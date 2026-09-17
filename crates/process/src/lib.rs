//! Processes and a wall-clock-free scheduler built on
//! `capability`'s `Kernel`. See `docs/design/KERNEL.md` and
//! `docs/design/decisions/ADR-002-processes-and-scheduling.md`.

mod process;
mod scheduler;

pub use process::{Handle, Process, ProcessId};
pub use scheduler::{Grant, ProcessError, Scheduler};
