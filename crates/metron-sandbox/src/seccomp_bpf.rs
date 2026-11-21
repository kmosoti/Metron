//! Pure-Rust `seccompiler` BPF deny-list for network sockets, namespaces, IPC, and high-risk syscalls.

use crate::error::SandboxError;
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch, apply_filter_all_threads,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(target_arch = "x86_64")]
const ARCH: TargetArch = TargetArch::x86_64;
#[cfg(target_arch = "aarch64")]
const ARCH: TargetArch = TargetArch::aarch64;

const DENIED_SYSCALLS: &[i64] = &[
    libc::SYS_ptrace,
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_pivot_root,
    libc::SYS_chroot,
    libc::SYS_bpf,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_userfaultfd,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_perf_event_open,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_personality,
    libc::SYS_kcmp,
    libc::SYS_syslog,
    libc::SYS_shmget,
    libc::SYS_msgget,
    libc::SYS_semget,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_reboot,
    libc::SYS_swapon,
    libc::SYS_swapoff,
];

/// Linux `CLONE_NEW*` flags blocked on `clone(flags, ...)` arg 0.
const CLONE_NS_FLAGS: &[u64] = &[
    libc::CLONE_NEWNS as u64,
    libc::CLONE_NEWUTS as u64,
    libc::CLONE_NEWIPC as u64,
    libc::CLONE_NEWUSER as u64,
    libc::CLONE_NEWPID as u64,
    libc::CLONE_NEWNET as u64,
    libc::CLONE_NEWCGROUP as u64,
    0x0000_0080, // CLONE_NEWTIME
];

const DENIED_SOCKET_FAMILIES: &[i32] = &[
    libc::AF_INET,
    libc::AF_INET6,
    libc::AF_NETLINK,
    libc::AF_PACKET,
    libc::AF_VSOCK,
    libc::AF_BLUETOOTH,
    libc::AF_ALG,
    libc::AF_XDP,
];

/// Configuration for the `seccomp-bpf` filter installed before workload execution.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SeccompPolicy {
    /// When `true` (Lane A post-load), denies `PROT_EXEC` on `mmap`, `mprotect`, and `pkey_mprotect`.
    #[serde(default)]
    pub deny_prot_exec: bool,
    /// When `false` (default), also blocks `socket(AF_UNIX, ...)` with `EPERM`.
    #[serde(default)]
    pub allow_unix_sockets: bool,
}

/// Builds the compiled BPF program returning `EPERM` on all blocked syscalls and socket families.
pub fn build_filter(policy: SeccompPolicy) -> Result<BpfProgram, SandboxError> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for &nr in DENIED_SYSCALLS {
        rules.insert(nr, vec![]);
    }

    // Block `clone(flags, ...)` whenever any `CLONE_NEW*` namespace bit is set,
    // while keeping normal thread/process `clone()` functional.
    for &flag in CLONE_NS_FLAGS {
        let cond = SeccompCondition::new(
            0,
            SeccompCmpArgLen::Qword,
            SeccompCmpOp::MaskedEq(flag),
            flag,
        )
        .map_err(|e| SandboxError::Denied(format!("seccomp clone ns cond: {e}")))?;
        let rule = SeccompRule::new(vec![cond])
            .map_err(|e| SandboxError::Denied(format!("seccomp clone ns rule: {e}")))?;
        rules.entry(libc::SYS_clone).or_default().push(rule);
    }

    for &family in DENIED_SOCKET_FAMILIES {
        push_socket_deny(&mut rules, family)?;
    }
    if !policy.allow_unix_sockets {
        push_socket_deny(&mut rules, libc::AF_UNIX)?;
    }

    if policy.deny_prot_exec {
        let prot_exec = libc::PROT_EXEC as u64;
        for nr in [libc::SYS_mmap, libc::SYS_mprotect, libc::SYS_pkey_mprotect] {
            let cond = SeccompCondition::new(
                2,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::MaskedEq(prot_exec),
                prot_exec,
            )
            .map_err(|e| SandboxError::Denied(format!("seccomp prot_exec cond: {e}")))?;
            let rule = SeccompRule::new(vec![cond])
                .map_err(|e| SandboxError::Denied(format!("seccomp prot_exec rule: {e}")))?;
            rules.entry(nr).or_default().push(rule);
        }
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EPERM as u32),
        ARCH,
    )
    .map_err(|e| SandboxError::Denied(format!("seccomp filter build: {e}")))?;

    filter
        .try_into()
        .map_err(|e| SandboxError::Denied(format!("seccomp BPF compile: {e}")))
}

fn push_socket_deny(
    rules: &mut BTreeMap<i64, Vec<SeccompRule>>,
    family: i32,
) -> Result<(), SandboxError> {
    let cond = SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, family as u64)
        .map_err(|e| SandboxError::Denied(format!("seccomp socket cond: {e}")))?;
    let rule = SeccompRule::new(vec![cond])
        .map_err(|e| SandboxError::Denied(format!("seccomp socket rule: {e}")))?;
    rules.entry(libc::SYS_socket).or_default().push(rule);
    Ok(())
}

/// Builds the `clone3 -> ENOSYS` BPF filter so glibc falls back cleanly to `clone()`,
/// where `CLONE_NEW*` flags are strictly blocked with `EPERM`.
pub fn build_clone3_enosys_filter() -> Result<BpfProgram, SandboxError> {
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    rules.insert(libc::SYS_clone3, vec![]);
    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
        ARCH,
    )
    .map_err(|e| SandboxError::Denied(format!("seccomp clone3 filter build: {e}")))?;
    filter
        .try_into()
        .map_err(|e| SandboxError::Denied(format!("seccomp clone3 BPF compile: {e}")))
}

/// Sets `PR_SET_NO_NEW_PRIVS` (if not already set) and installs the stacked seccomp BPF filters
/// (`clone3 -> ENOSYS` plus the full deny-list returning `EPERM`).
pub fn install_seccomp(policy: SeccompPolicy) -> Result<(), SandboxError> {
    rustix::thread::set_no_new_privs(true)
        .map_err(|e| SandboxError::Denied(format!("PR_SET_NO_NEW_PRIVS failed: {e}")))?;
    let clone3_program = build_clone3_enosys_filter()?;
    apply_filter_all_threads(&clone3_program)
        .map_err(|e| SandboxError::Denied(format!("seccomp clone3 apply failed: {e}")))?;
    let program = build_filter(policy)?;
    apply_filter_all_threads(&program)
        .map_err(|e| SandboxError::Denied(format!("seccomp apply failed: {e}")))
}
