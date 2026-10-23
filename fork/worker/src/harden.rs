//! Process hardening at start-up, and the `doctor` report.

#![allow(unsafe_code, reason = "prctl, setrlimit and Win32 process calls")]

use std::fmt::Write as _;

/// What the host offers, for the start-up log and `ente-worker doctor`.
#[derive(Debug)]
pub struct Report {
    pub memfd_secret: Result<(), String>,
    pub dumpable_off: Result<(), String>,
    pub core_dumps_off: Result<(), String>,
    /// Soft memlock limit in bytes after raising it (None = unlimited).
    pub memlock_limit: Option<u64>,
    pub wsl: bool,
    pub swap_active: Option<bool>,
    pub ptrace_scope: Option<u8>,
}

impl Default for Report {
    fn default() -> Self {
        let unknown = || Err("not checked".to_owned());
        Self {
            memfd_secret: unknown(),
            dumpable_off: unknown(),
            core_dumps_off: unknown(),
            memlock_limit: None,
            wsl: false,
            swap_active: None,
            ptrace_scope: None,
        }
    }
}

impl Report {
    pub fn render(&self) -> String {
        let mut s = String::new();
        let ok = |r: &Result<(), String>| match r {
            Ok(()) => "yes".to_owned(),
            Err(e) => format!("NO ({e})"),
        };
        let _ = writeln!(s, "memfd_secret available:   {}", ok(&self.memfd_secret));
        let _ = writeln!(s, "not dumpable / ptraceable: {}", ok(&self.dumpable_off));
        let _ = writeln!(s, "core dumps disabled:      {}", ok(&self.core_dumps_off));
        let _ = writeln!(
            s,
            "memlock limit:            {}",
            self.memlock_limit
                .map_or_else(|| "unlimited".to_owned(), |b| format!("{} KiB", b / 1024))
        );
        if let Some(p) = self.ptrace_scope {
            let _ = writeln!(
                s,
                "yama ptrace_scope:        {p}{}",
                if p >= 2 {
                    ""
                } else {
                    " (2 or more recommended)"
                }
            );
        }
        if let Some(swap) = self.swap_active {
            let _ = writeln!(
                s,
                "swap active:              {}",
                if swap {
                    "yes (fine: keys are locked out of it)"
                } else {
                    "no"
                }
            );
        }
        if self.wsl {
            let _ = writeln!(
                s,
                "WSL2:                     yes. Windows may page the VM's memory (vmmem) to \
                 pagefile.sys regardless of mlock; keep BitLocker on and hibernation off \
                 (powercfg /h off)."
            );
        }
        s
    }
}

/// Applies every hardening step this OS supports and reports the result.
/// `locked_bytes` is how much secret memory the worker will lock.
pub fn harden(locked_bytes: usize) -> Report {
    let mut report = Report {
        memfd_secret: crate::secure_mem::memfd_secret_available().map_err(|e| e.to_string()),
        ..Report::default()
    };
    platform(&mut report, locked_bytes);
    report
}

#[cfg(target_os = "linux")]
fn platform(report: &mut Report, locked_bytes: usize) {
    use std::io;

    // SAFETY: prctl with integer arguments only.
    report.dumpable_off = if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().to_string())
    };
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: setrlimit reads the struct we pass.
    report.core_dumps_off = if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().to_string())
    };
    report.memlock_limit = raise_memlock(locked_bytes);

    let osrelease = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    report.wsl = osrelease.to_ascii_lowercase().contains("microsoft");
    report.swap_active = std::fs::read_to_string("/proc/swaps")
        .ok()
        .map(|s| s.lines().skip(1).any(|l| !l.trim().is_empty()));
    report.ptrace_scope = std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
        .ok()
        .and_then(|s| s.trim().parse().ok());
}

#[cfg(target_os = "linux")]
fn raise_memlock(locked_bytes: usize) -> Option<u64> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit fills the struct we pass.
    if unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut lim) } != 0 {
        return Some(0);
    }
    let want = u64::try_from(locked_bytes)
        .unwrap_or(u64::MAX)
        .saturating_mul(2);
    if lim.rlim_cur != libc::RLIM_INFINITY && lim.rlim_cur < want {
        let raised = libc::rlimit {
            rlim_cur: if lim.rlim_max == libc::RLIM_INFINITY {
                want
            } else {
                want.min(lim.rlim_max)
            },
            rlim_max: lim.rlim_max,
        };
        // SAFETY: setrlimit reads the struct we pass.
        if unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &raised) } == 0 {
            lim = raised;
        }
    }
    (lim.rlim_cur != libc::RLIM_INFINITY).then_some(lim.rlim_cur)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn platform(report: &mut Report, _locked_bytes: usize) {
    let zero = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: setrlimit reads the struct we pass.
    report.core_dumps_off = if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    };
    report.dumpable_off = Err("not supported on this OS".to_owned());
}

#[cfg(windows)]
fn platform(report: &mut Report, locked_bytes: usize) {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};

    // No WER dialog or dump for this process; machine-wide WER policy for the
    // service account is set up as in fork/worker/README.md.
    // SAFETY: SetErrorMode takes flags only.
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
    report.core_dumps_off = Err("set WER LocalDumps DumpCount=0 for the service".to_owned());
    report.dumpable_off = Err("run as a dedicated service account".to_owned());

    // VirtualLock is limited by the minimum working set: raise it to fit.
    let min = 16 * 1024 * 1024 + locked_bytes.saturating_mul(2);
    let max = min.saturating_mul(4);
    // SAFETY: plain call on our own process handle.
    let ok = unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), min, max) } != 0;
    report.memlock_limit = ok.then_some(u64::try_from(min).unwrap_or(u64::MAX));
}

/// Logs the report: missing protections are warnings, not silence.
pub fn log_report(report: &Report) {
    match &report.memfd_secret {
        Ok(()) => log::info!("secret memory: memfd_secret"),
        Err(e) => log::warn!(
            "memfd_secret is NOT available ({e}); keys are kept in mlock'ed pages, which \
             stay out of swap but are visible in the kernel's direct map"
        ),
    }
    if let Err(e) = &report.dumpable_off {
        log::warn!("could not make the process non-dumpable: {e}");
    }
    if let Err(e) = &report.core_dumps_off {
        log::warn!("core dumps not disabled: {e}");
    }
    if report.wsl {
        log::warn!(
            "running under WSL2: Windows may page the VM's memory to pagefile.sys \
             regardless of mlock. Keep BitLocker on and hibernation off."
        );
    }
    if matches!(report.ptrace_scope, Some(p) if p < 2) {
        log::warn!(
            "kernel.yama.ptrace_scope is below 2: same-user processes can attach to the worker"
        );
    }
}
