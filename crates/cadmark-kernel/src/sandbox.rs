// Confinement of the kernel worker process. The build123d scripts CADmark
// runs are machine-generated and re-executed every session, so the process
// that runs them is boxed before the interpreter starts: it can read the
// Python runtime and the system directories, read and write the project
// folder and its own scratch directory, and nothing else (a kernel older
// than Linux 6.2 cannot deny truncation, see `REQUIRED_ABI`); it cannot
// open a socket; and it can never regain privileges. The parent process supplies
// no environment, so no credential is reachable from executed code.
//
// Confinement fails closed: when the running kernel cannot enforce it, the
// worker refuses to run scripts rather than run them unconfined. Wall-clock
// and memory ceilings are enforced from the parent (see `worker.rs`), which
// can kill a process that has stopped cooperating.
//
// Linux only: Landlock (filesystem) and seccomp-BPF (network) are the
// mechanisms.

use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus, path_beneath_rules,
};
use thiserror::Error;

/// What the worker may touch. Everything not listed is denied, as far as
/// the running kernel's Landlock ABI reaches (see `REQUIRED_ABI`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicy {
    /// The project folder: scripts, exports, reference images. Read-write.
    pub project_dir: PathBuf,
    /// The worker's own scratch directory (model files). Read-write.
    pub scratch_dir: PathBuf,
    /// The Python runtime and its packages: the interpreter home and the
    /// virtual environment. Read and execute only.
    pub runtime_roots: Vec<PathBuf>,
}

/// System directories every Python process needs to read: shared
/// libraries, locale and certificate data, and the null and random devices.
const SYSTEM_READ_ROOTS: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/etc",
    "/dev/null",
    "/dev/urandom",
    "/dev/random",
    "/dev/zero",
];

/// The Landlock ABI whose access set the policy is written against. ABI 3
/// (Linux 6.2) adds truncation. An older kernel enforces every right it
/// knows, so reads and writes outside the policy are still denied, but it
/// cannot deny truncating a file outside it; that confinement is reported
/// as partial rather than refused.
const REQUIRED_ABI: ABI = ABI::V3;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("the kernel cannot confine the script sandbox (Landlock is not enforced): {0}")]
    LandlockUnavailable(String),
    #[error("could not build the script sandbox: {0}")]
    Landlock(#[from] landlock::RulesetError),
    #[error("could not deny network access to the script sandbox: {0}")]
    Seccomp(std::io::Error),
}

/// How thoroughly the calling process is confined, for the log line the
/// worker prints on start-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// Every rule of the policy is enforced.
    Full,
    /// The filesystem policy is enforced as far as the running kernel's
    /// Landlock ABI reaches. The kernel predates the truncation right, so
    /// truncating a file outside the policy is not denied.
    Partial,
}

/// Confine the calling process, permanently. Call once, on the main
/// thread, before the Python interpreter initialises: threads created
/// afterwards inherit the confinement.
pub fn confine(policy: &SandboxPolicy) -> Result<Confinement, SandboxError> {
    let confinement = restrict_filesystem(policy)?;
    deny_network().map_err(SandboxError::Seccomp)?;
    Ok(confinement)
}

fn restrict_filesystem(policy: &SandboxPolicy) -> Result<Confinement, SandboxError> {
    let read_roots: Vec<&Path> = SYSTEM_READ_ROOTS
        .iter()
        .map(Path::new)
        .chain(policy.runtime_roots.iter().map(PathBuf::as_path))
        .collect();
    let write_roots = [policy.project_dir.as_path(), policy.scratch_dir.as_path()];

    let status = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(REQUIRED_ABI))?
        .create()?
        .add_rules(path_beneath_rules(
            read_roots,
            AccessFs::from_read(REQUIRED_ABI),
        ))?
        .add_rules(path_beneath_rules(
            write_roots,
            AccessFs::from_all(REQUIRED_ABI),
        ))?
        .restrict_self()?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(Confinement::Full),
        RulesetStatus::PartiallyEnforced => Ok(Confinement::Partial),
        RulesetStatus::NotEnforced => Err(SandboxError::LandlockUnavailable(format!(
            "{:?}",
            status.landlock
        ))),
    }
}

// ── Network denial ────────────────────────────────────────────────────
//
// A seccomp-BPF filter that fails socket creation with EPERM. Landlock
// governs the filesystem and (from ABI 4) TCP ports, but a script that
// cannot create a socket of any family cannot reach the network by any
// route, so the syscall filter is the boundary.

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;

/// Syscalls executed code may not make. Creating a socket is the one that
/// matters; the rest close the door on a socket inherited by accident.
const DENIED_SYSCALLS: &[libc::c_long] = &[
    libc::SYS_socket,
    libc::SYS_socketpair,
    libc::SYS_connect,
    libc::SYS_bind,
    libc::SYS_listen,
    libc::SYS_accept,
    libc::SYS_accept4,
];

fn bpf(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

/// The filter program: refuse to run under a foreign architecture, then
/// return EPERM for each denied syscall number and allow everything else.
fn network_denial_program() -> Vec<libc::sock_filter> {
    let nr_offset = std::mem::offset_of!(libc::seccomp_data, nr) as u32;
    let arch_offset = std::mem::offset_of!(libc::seccomp_data, arch) as u32;
    let denied = DENIED_SYSCALLS.len() as u8;

    let mut program = vec![
        bpf(
            libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
            0,
            0,
            arch_offset,
        ),
        bpf(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            1,
            0,
            AUDIT_ARCH,
        ),
        bpf(
            libc::BPF_RET | libc::BPF_K,
            0,
            0,
            libc::SECCOMP_RET_KILL_PROCESS,
        ),
        bpf(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, nr_offset),
    ];
    // Each comparison jumps straight to the EPERM return when it matches
    // and falls through to the next comparison otherwise; the last one
    // falls through to the allow return.
    for (index, syscall) in DENIED_SYSCALLS.iter().enumerate() {
        let remaining = denied - index as u8 - 1;
        program.push(bpf(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            remaining + 1,
            0,
            *syscall as u32,
        ));
    }
    program.push(bpf(
        libc::BPF_RET | libc::BPF_K,
        0,
        0,
        libc::SECCOMP_RET_ALLOW,
    ));
    program.push(bpf(
        libc::BPF_RET | libc::BPF_K,
        0,
        0,
        libc::SECCOMP_RET_ERRNO | libc::EPERM as u32,
    ));
    program
}

fn deny_network() -> Result<(), std::io::Error> {
    let mut program = network_denial_program();
    let prog = libc::sock_fprog {
        len: program.len() as libc::c_ushort,
        filter: program.as_mut_ptr(),
    };
    // `no_new_privs` is already set by the Landlock enforcement above;
    // seccomp refuses to install a filter without it.
    // SAFETY: `prog` points at a filter that outlives the call, and
    // SECCOMP_SET_MODE_FILTER with TSYNC is the documented way to apply a
    // filter to every thread of the calling process.
    let result = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            libc::SECCOMP_FILTER_FLAG_TSYNC,
            &prog as *const libc::sock_fprog,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk the filter as the kernel would for one syscall number.
    fn evaluate(program: &[libc::sock_filter], nr: u32, arch: u32) -> u32 {
        let mut accumulator = 0u32;
        let mut pc = 0usize;
        loop {
            let instruction = &program[pc];
            let code = u32::from(instruction.code);
            if code == libc::BPF_LD | libc::BPF_W | libc::BPF_ABS {
                accumulator = if instruction.k == 0 { nr } else { arch };
                pc += 1;
            } else if code == libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K {
                pc += 1 + usize::from(if accumulator == instruction.k {
                    instruction.jt
                } else {
                    instruction.jf
                });
            } else if code == libc::BPF_RET | libc::BPF_K {
                return instruction.k;
            } else {
                panic!("unexpected BPF opcode {code:#x}");
            }
        }
    }

    #[test]
    fn socket_syscalls_are_refused_with_eperm_and_others_allowed() {
        let program = network_denial_program();
        let eperm = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
        for denied in DENIED_SYSCALLS {
            assert_eq!(evaluate(&program, *denied as u32, AUDIT_ARCH), eperm);
        }
        for allowed in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_openat,
            libc::SYS_mmap,
        ] {
            assert_eq!(
                evaluate(&program, allowed as u32, AUDIT_ARCH),
                libc::SECCOMP_RET_ALLOW
            );
        }
    }

    #[test]
    fn a_foreign_architecture_is_killed_before_any_syscall_is_read() {
        let program = network_denial_program();
        assert_eq!(
            evaluate(&program, libc::SYS_read as u32, AUDIT_ARCH ^ 1),
            libc::SECCOMP_RET_KILL_PROCESS
        );
    }

    #[test]
    fn every_jump_lands_inside_the_program() {
        let program = network_denial_program();
        // An instruction's class is its low three bits.
        let is_jump = |code: u16| u32::from(code) & 0x07 == libc::BPF_JMP;
        for (index, instruction) in program.iter().enumerate() {
            if is_jump(instruction.code) {
                assert!(index + 1 + usize::from(instruction.jt) < program.len());
                assert!(index + 1 + usize::from(instruction.jf) < program.len());
            }
        }
    }
}
