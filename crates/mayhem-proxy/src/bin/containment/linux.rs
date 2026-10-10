//! Dedicated decoder only: no provider-controlled syscall rules or executable.
//! Linux seccomp/no_new_privs documentation and syscall ABI references are in
//! WORKER_CONTAINMENT.md. Unsafe calls stay outside the protocol/financial lib.
use libc::{c_long, sock_filter, sock_fprog};

const ALLOW: u32 = libc::SECCOMP_RET_ALLOW;
const DENY: u32 = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
const KILL: u32 = libc::SECCOMP_RET_KILL_PROCESS;
#[cfg(target_arch = "x86_64")]
const ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const ARCH: u32 = 0xc000_00b7;

pub(super) fn enter() -> Result<(), ()> {
    // Only the trusted dedicated child's single-threaded startup may call this.
    // No Rust object owns inherited descriptors. stdio belongs to the broker.
    close_inherited()?;
    install(&filter())
}

fn close_inherited() -> Result<(), ()> {
    // Enumerate actual descriptors, not a caller-controlled rlimit or a guessed
    // numerical ceiling. Fail closed if procfs is absent/inaccessible. The
    // iterator closes its own descriptor before we touch the collected numbers.
    let inherited = std::fs::read_dir("/proc/self/fd")
        .map_err(|_| ())?
        .map(|entry| {
            entry
                .map_err(|_| ())?
                .file_name()
                .to_str()
                .ok_or(())?
                .parse::<libc::c_int>()
                .map_err(|_| ())
        })
        .collect::<Result<Vec<_>, ()>>()?;
    for fd in inherited.into_iter().filter(|fd| *fd > 2) {
        // SAFETY: startup owns the process; no live Rust owner or other thread
        // can reuse these inherited handles. Confirm even an interrupted close.
        unsafe {
            libc::close(fd);
        }
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } != -1
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::EBADF)
        {
            return Err(());
        }
    }
    Ok(())
}

fn install(instructions: &[sock_filter]) -> Result<(), ()> {
    let length = u16::try_from(instructions.len()).map_err(|_| ())?;
    let program = sock_fprog {
        len: length,
        filter: instructions.as_ptr().cast_mut(),
    };
    let mut action = KILL;
    // SAFETY: typed kernel ABI structs/pointers live through synchronous calls;
    // kernel copies the filter, never mutates it. All variadic integer arguments
    // use machine-long widths. No untrusted bytes select flags or instructions.
    let installed = unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1 as c_long, 0 as c_long, 0 as c_long, 0 as c_long) == 0
            // Require Linux >=4.14 semantics rather than silently downgrading
            // architecture violations to a kill of only the calling thread.
            && libc::syscall(libc::SYS_seccomp, libc::SECCOMP_GET_ACTION_AVAIL as c_long,
                0 as c_long, &mut action) == 0
            && libc::syscall(libc::SYS_seccomp, libc::SECCOMP_SET_MODE_FILTER as c_long,
                libc::SECCOMP_FILTER_FLAG_TSYNC as c_long, &program) == 0
    };
    if installed {
        Ok(())
    } else {
        Err(())
    }
}

fn op(code: u16, k: u32) -> sock_filter {
    sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}
fn load(offset: u32) -> sock_filter {
    op((libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16, offset)
}
fn ret(value: u32) -> sock_filter {
    op((libc::BPF_RET | libc::BPF_K) as u16, value)
}
fn eq(value: u32, yes: u8, no: u8) -> sock_filter {
    sock_filter {
        code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
        jt: yes,
        jf: no,
        k: value,
    }
}
fn equal(arg: u32, value: u32) -> Vec<sock_filter> {
    masked(arg, u32::MAX, value)
}
fn masked(arg: u32, mask: u32, value: u32) -> Vec<sock_filter> {
    // These arguments have kernel int/u32 semantics. Compare the low word as
    // the kernel does; ignored upper bits cannot turn a denied fd/flag into one.
    vec![
        load(16 + arg * 8),
        op((libc::BPF_ALU | libc::BPF_AND | libc::BPF_K) as u16, mask),
        eq(value, 1, 0),
        ret(DENY),
    ]
}
fn one_of(arg: u32, values: &[u32]) -> Vec<sock_filter> {
    let mut block = vec![load(16 + arg * 8)];
    for (i, value) in values.iter().enumerate() {
        block.push(eq(*value, (values.len() - i) as u8, 0));
    }
    block.push(ret(DENY));
    block
}
fn rule(code: &mut Vec<sock_filter>, syscall: c_long, mut guards: Vec<sock_filter>) {
    guards.push(ret(ALLOW));
    code.push(eq(
        syscall as u32,
        0,
        u8::try_from(guards.len()).expect("static filter block"),
    ));
    code.extend(guards);
}

fn filter() -> Vec<sock_filter> {
    let mut code = vec![load(4), eq(ARCH, 1, 0), ret(KILL), load(0)];
    #[cfg(target_arch = "x86_64")]
    {
        // x32 shares AUDIT_ARCH_X86_64. Never permit its syscall-number bit.
        code.push(sock_filter {
            code: (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16,
            jt: 0,
            jf: 1,
            k: 0x4000_0000,
        });
        code.push(ret(KILL));
    }
    for nr in [libc::SYS_read, libc::SYS_readv] {
        rule(&mut code, nr, equal(0, 0));
    }
    for nr in [libc::SYS_write, libc::SYS_writev] {
        rule(&mut code, nr, one_of(0, &[1, 2]));
    }
    for nr in [libc::SYS_close, libc::SYS_fstat] {
        rule(&mut code, nr, one_of(0, &[0, 1, 2]));
    }
    let mut fcntl = one_of(0, &[0, 1, 2]);
    fcntl.extend(one_of(1, &[libc::F_GETFD as u32, libc::F_GETFL as u32]));
    rule(&mut code, libc::SYS_fcntl, fcntl);
    let protection = (libc::PROT_READ | libc::PROT_WRITE) as u32;
    let mut mmap = masked(2, !protection, 0);
    mmap.extend(masked(
        3,
        (libc::MAP_TYPE | libc::MAP_ANONYMOUS) as u32,
        (libc::MAP_PRIVATE | libc::MAP_ANONYMOUS) as u32,
    ));
    let flags = (libc::MAP_PRIVATE
        | libc::MAP_ANONYMOUS
        | libc::MAP_FIXED
        | libc::MAP_NORESERVE
        | libc::MAP_STACK
        | libc::MAP_FIXED_NOREPLACE) as u32;
    mmap.extend(masked(3, !flags, 0));
    mmap.extend(equal(4, u32::MAX));
    rule(&mut code, libc::SYS_mmap, mmap);
    rule(&mut code, libc::SYS_mprotect, masked(2, !protection, 0));
    rule(
        &mut code,
        libc::SYS_mremap,
        masked(3, !(libc::MREMAP_MAYMOVE | libc::MREMAP_FIXED) as u32, 0),
    );
    rule(
        &mut code,
        libc::SYS_madvise,
        one_of(
            2,
            &[
                libc::MADV_DONTNEED as u32,
                libc::MADV_FREE as u32,
                libc::MADV_DONTDUMP as u32,
            ],
        ),
    );
    rule(
        &mut code,
        libc::SYS_futex,
        one_of(
            1,
            &[
                (libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG) as u32,
                (libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG) as u32,
                (libc::FUTEX_WAIT_BITSET | libc::FUTEX_PRIVATE_FLAG) as u32,
                (libc::FUTEX_WAIT_BITSET | libc::FUTEX_PRIVATE_FLAG | libc::FUTEX_CLOCK_REALTIME)
                    as u32,
            ],
        ),
    );
    for nr in [libc::SYS_clock_gettime, libc::SYS_clock_getres] {
        rule(&mut code, nr, one_of(0, &[0, 1, 2, 3, 4, 5, 6, 7]));
    }
    rule(
        &mut code,
        libc::SYS_getrandom,
        // Rust uses INSECURE only for HashMap seeding. These flags expose no
        // files or descriptors; cryptographic getrandom(0) remains unchanged.
        one_of(
            2,
            &[0, libc::GRND_NONBLOCK as u32, libc::GRND_INSECURE as u32],
        ),
    );
    for nr in [
        libc::SYS_brk,
        libc::SYS_munmap,
        libc::SYS_rt_sigaction,
        libc::SYS_rt_sigprocmask,
        libc::SYS_rt_sigreturn,
        libc::SYS_sigaltstack,
        libc::SYS_getpid,
        libc::SYS_gettid,
        libc::SYS_sched_yield,
        libc::SYS_nanosleep,
        libc::SYS_gettimeofday,
        libc::SYS_exit,
        libc::SYS_exit_group,
    ] {
        rule(&mut code, nr, Vec::new());
    }
    // No opens, path metadata, sockets/IPC creation or fd passing, executable
    // mappings, fork/clone/exec, process control, ptrace, io_uring or new syscalls.
    code.push(ret(DENY));
    code
}

#[cfg(test)]
#[path = "linux/tests.rs"]
mod tests;
