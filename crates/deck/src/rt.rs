//! Realtime setup on Linux: the audio thread on `SCHED_FIFO`, and memory
//! locked so the callback never waits on a page fault. Both need limits the
//! systemd unit grants (`LimitRTPRIO`, `LimitMEMLOCK`); without them the
//! deck still runs, just with less protection from everything else.

/// Priority for the audio thread: above ordinary threaded IRQs (50 on
/// `PREEMPT_RT`), below the kernel's own critical threads (99).
#[cfg(target_os = "linux")]
const AUDIO_PRIORITY: i32 = 80;

/// What became of the audio thread's promotion, for the UI to report.
pub const UNKNOWN: i32 = 0;
/// Promoted; anything else is `-errno`.
pub const PROMOTED: i32 = 1;

/// Put the calling thread on `SCHED_FIFO`. One syscall: fine to call once
/// from the audio callback itself. Returns `PROMOTED` or `-errno`.
#[cfg(target_os = "linux")]
pub fn promote_current_thread() -> i32 {
    let param = libc::sched_param { sched_priority: AUDIO_PRIORITY };
    // SAFETY: `pthread_self` is always valid for the calling thread, and
    // `param` outlives the call.
    let err = unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &raw const param) };
    if err == 0 { PROMOTED } else { -err }
}

#[cfg(not(target_os = "linux"))]
pub const fn promote_current_thread() -> i32 {
    // Core Audio already runs its callback on a realtime thread.
    PROMOTED
}

/// Lock all memory, now and future, if the limit allows all of it (a lower
/// limit would make big allocations, like loading a track, fail instead).
#[cfg(target_os = "linux")]
pub fn lock_memory() -> Result<(), String> {
    let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: `lim` is a valid, writable rlimit for the duration of the call.
    if unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &raw mut lim) } != 0 || lim.rlim_cur != libc::RLIM_INFINITY {
        return Err("the memlock limit isn't unlimited (LimitMEMLOCK=infinity)".into());
    }
    // SAFETY: plain syscall with constant flags.
    if unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn lock_memory() -> Result<(), String> {
    Err("only on Linux".into())
}

/// A `promote_current_thread` result, for the log.
pub fn describe(status: i32) -> String {
    match status {
        PROMOTED => "realtime priority".into(),
        UNKNOWN => "not started".into(),
        e => format!(
            "no realtime priority ({}; needs LimitRTPRIO)",
            std::io::Error::from_raw_os_error(e.saturating_neg())
        ),
    }
}
