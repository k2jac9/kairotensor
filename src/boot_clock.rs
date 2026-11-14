//! Restart-stable local Linux clock domain, not hardware attestation.
//! Assumes trusted kernel/procfs and machine identity. VM rollback/cloning and
//! hostile root are outside this contract; namespace inode reuse is not attested.
use crate::RecoveryClock;
use std::io;
/// Samples Linux CLOCK_BOOTTIME, including suspend, without a wall-clock fallback.
/// Missing or ambiguous current-thread namespace metadata is an error.
/// Re-read on every sample, including after process restart. No host identifiers
/// are logged by this provider; the returned domain is caller-persisted metadata.
pub struct LinuxBootClock;
impl LinuxBootClock {
    #[cfg(target_os = "linux")]
    pub fn sample() -> io::Result<RecoveryClock> {
        checked_sample(read_identity, || {
            let mut t = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: clock_gettime writes a valid timespec to this initialized object.
            // CLOCK_BOOTTIME is Linux-specific and selected only on Linux.
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut t) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok((t.tv_sec as i64, t.tv_nsec as i64))
        })
    }
    #[cfg(not(target_os = "linux"))]
    pub fn sample() -> io::Result<RecoveryClock> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux boot clock unsupported on this platform",
        ))
    }
}
fn bad() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "unavailable or ambiguous Linux clock domain",
    )
}
#[cfg(target_os = "linux")]
fn read_identity() -> io::Result<String> {
    use std::fs::{read_link, read_to_string};
    let machine = read_to_string("/etc/machine-id")?;
    let boot = read_to_string("/proc/sys/kernel/random/boot_id")?;
    // thread-self is essential: a process's threads can have different namespaces.
    let active = read_link("/proc/thread-self/ns/time")?;
    let children = read_link("/proc/thread-self/ns/time_for_children")?;
    // This proc file describes time_for_children, hence the equality requirement.
    let offsets = read_to_string("/proc/thread-self/timens_offsets")?;
    domain(
        machine.trim(),
        boot.trim(),
        active.to_str().ok_or_else(bad)?,
        children.to_str().ok_or_else(bad)?,
        &offsets,
    )
}
fn domain(
    machine: &str,
    boot: &str,
    active: &str,
    children: &str,
    offsets: &str,
) -> io::Result<String> {
    let hex = |s: &str| {
        s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    };
    if machine.len() != 32 || !hex(machine) || machine.bytes().all(|c| c == b'0') {
        return Err(bad());
    }
    if boot.len() != 36
        || !boot.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
        || boot.bytes().all(|c| c == b'0' || c == b'-')
    {
        return Err(bad());
    }
    if active != children {
        return Err(bad());
    }
    let inode = active
        .strip_prefix("time:[")
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(bad)?;
    if inode.is_empty()
        || !inode.bytes().all(|c| c.is_ascii_digit())
        || inode.parse::<u64>().map_err(|_| bad())? == 0
    {
        return Err(bad());
    }
    let mut values = [None, None];
    for line in offsets.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 3 {
            return Err(bad());
        }
        let index = match parts[0] {
            "monotonic" => 0,
            "boottime" => 1,
            _ => return Err(bad()),
        };
        let sec = parts[1].parse::<i64>().map_err(|_| bad())?;
        let ns = parts[2].parse::<i64>().map_err(|_| bad())?;
        if values[index].is_some() || !(0..1_000_000_000).contains(&ns) {
            return Err(bad());
        }
        values[index] = Some((sec, ns));
    }
    let (ms, mn) = values[0].ok_or_else(bad)?;
    let (bs, bn) = values[1].ok_or_else(bad)?;
    Ok(format!(
        "linux-boottime/v1/{machine}/{boot}/{active}/monotonic:{ms}:{mn}/boottime:{bs}:{bn}"
    ))
}
fn milliseconds(sec: i64, nsec: i64) -> io::Result<i64> {
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(bad());
    }
    sec.checked_mul(1000)
        .and_then(|v| v.checked_add((nsec + 999_999) / 1_000_000))
        .ok_or_else(bad)
}
fn checked_sample<I, C>(mut identity: I, time: C) -> io::Result<RecoveryClock>
where
    I: FnMut() -> io::Result<String>,
    C: FnOnce() -> io::Result<(i64, i64)>,
{
    let before = identity()?;
    let (sec, nsec) = time()?;
    let now_ms = milliseconds(sec, nsec)?;
    if identity()? != before {
        return Err(bad());
    }
    Ok(RecoveryClock {
        domain: before,
        now_ms,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    const MACHINE: &str = "0123456789abcdef0123456789abcdef";
    const BOOT: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const OFFSETS: &str = "monotonic 0 0\nboottime 0 0\n";
    #[test]
    fn identity_is_stable_and_binds_all_domains() {
        let first = domain(MACHINE, BOOT, "time:[42]", "time:[42]", OFFSETS).unwrap();
        assert_eq!(
            first,
            domain(MACHINE, BOOT, "time:[42]", "time:[42]", OFFSETS).unwrap()
        );
        for d in [
            domain(
                "abcdef0123456789abcdef0123456789",
                BOOT,
                "time:[42]",
                "time:[42]",
                OFFSETS,
            ),
            domain(
                MACHINE,
                "abcdef01-89ab-cdef-0123-456789abcdef",
                "time:[42]",
                "time:[42]",
                OFFSETS,
            ),
            domain(MACHINE, BOOT, "time:[43]", "time:[43]", OFFSETS),
            domain(
                MACHINE,
                BOOT,
                "time:[42]",
                "time:[42]",
                "monotonic 0 0\nboottime 1 0\n",
            ),
        ] {
            assert_ne!(first, d.unwrap());
        }
    }
    #[test]
    fn invalid_or_ambiguous_identity_refuses() {
        for machine in ["", "unknown", "00000000000000000000000000000000", "abc"] {
            assert!(domain(machine, BOOT, "time:[42]", "time:[42]", OFFSETS).is_err());
        }
        for boot in ["", "not-a-uuid", "00000000-0000-0000-0000-000000000000"] {
            assert!(domain(MACHINE, boot, "time:[42]", "time:[42]", OFFSETS).is_err());
        }
        for (active, children) in [
            ("time:[42]", "time:[43]"),
            ("time:[0]", "time:[0]"),
            ("bad", "bad"),
        ] {
            assert!(domain(MACHINE, BOOT, active, children, OFFSETS).is_err());
        }
        for offsets in [
            "",
            "boottime 0 0",
            "monotonic 0 0\nboottime 0 1000000000",
            "monotonic 0 0\nboottime 0 0\nboottime 1 0",
            "monotonic zero 0\nboottime 0 0",
        ] {
            assert!(domain(MACHINE, BOOT, "time:[42]", "time:[42]", offsets).is_err());
        }
    }
    #[test]
    fn conversion_rounds_up_and_rejects_invalid_or_overflow() {
        assert_eq!(milliseconds(1, 1).unwrap(), 1001);
        assert_eq!(milliseconds(1, 0).unwrap(), 1000);
        assert_eq!(milliseconds(0, 999999999).unwrap(), 1000);
        for (s, n) in [(-1, 0), (0, -1), (0, 1000000000), (i64::MAX, 0)] {
            assert!(milliseconds(s, n).is_err());
        }
    }
    #[test]
    fn sample_requires_identity_continuity() {
        let sample = checked_sample(|| Ok("fixture".into()), || Ok((1, 1))).unwrap();
        assert_eq!(sample.now_ms, 1001);
        let mut n = 0;
        assert!(checked_sample(
            || {
                n += 1;
                Ok(n.to_string())
            },
            || Ok((1, 0))
        )
        .is_err());
        assert!(checked_sample(
            || Err(io::Error::other("unavailable")),
            || panic!("no clock fallback")
        )
        .is_err());
        assert!(checked_sample(
            || Ok("fixture".into()),
            || Err(io::Error::other("clock failed"))
        )
        .is_err());
    }
}
