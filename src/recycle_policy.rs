//! Pure Recycle Bin eligibility rules.
//!
//! The Windows Shell, not DiskScope, decides whether a file is recycled. When it
//! cannot recycle a file it deletes it, and no documented flag turns that into a
//! failure. These rules therefore refuse to *ask* the Shell to recycle a file
//! only when there is positive evidence that the Shell would delete it:
//!
//! * the drive is not a fixed local disk (USB flash drives and memory cards
//!   normally keep no Recycle Bin),
//! * a policy turns the Recycle Bin off,
//! * the volume is set to "Remove files immediately", or
//! * the file is at least as large as the volume's Recycle Bin maximum.
//!
//! Missing registry values are Windows defaults and never a reason to refuse.
//! A value or API call that fails is missing evidence: it downgrades the verdict
//! to [`RecycleVerdict::Unverified`] but never blocks. Gathering the facts is the
//! caller's job (see `file_actions`); everything here is a total function over
//! plain data so it can be tested on any platform.

/// Bytes per MiB. The Bin maximum is stored in MiB.
pub const MIB: u64 = 1 << 20;
/// `GetDriveTypeW` result for a local fixed disk.
pub const DRIVE_FIXED: u32 = 3;
/// Windows' default Bin maximum is 10% of the first 40 GiB and 5% of the rest.
const DEFAULT_FIRST_TIER: u64 = 40 << 30;

/// One registry DWORD as observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// The key or value does not exist: the Windows default applies.
    Absent,
    Value(u32),
    /// The lookup failed or had an unexpected type: no evidence either way.
    Unreadable,
}

impl Setting {
    /// The stricter of two observations: a nonzero value beats an unreadable
    /// one, which beats a zero value, which beats an absent one.
    pub fn combine(self, other: Self) -> Self {
        let rank = |setting: Self| match setting {
            Self::Value(0) => 1,
            Self::Value(_) => 3,
            Self::Unreadable => 2,
            Self::Absent => 0,
        };
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
    pub fn nonzero(self) -> bool {
        matches!(self, Self::Value(value) if value != 0)
    }
    fn unreadable(self) -> bool {
        self == Self::Unreadable
    }
}

/// Everything the verdict depends on, gathered by the platform layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecycleFacts {
    /// `GetDriveTypeW` for the file's drive root.
    pub drive_type: u32,
    /// `NoRecycleFiles` policy, strictest of machine and user.
    pub policy_no_recycle: Setting,
    /// Whether the volume GUID (and so the per-volume settings) was resolved.
    pub volume_resolved: bool,
    /// Per-volume "Remove files immediately".
    pub volume_nuke_on_delete: Setting,
    /// Per-volume Bin maximum in MiB.
    pub volume_max_capacity_mb: Setting,
    /// Volume capacity for the default maximum, when known.
    pub volume_total_bytes: Option<u64>,
}

/// Why Recycle is refused before the Shell is involved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecycleBlock {
    NoBinOnDrive,
    DisabledByPolicy,
    DeletesImmediately,
    TooLarge {
        size: u64,
        limit: u64,
    },
    /// The Shell itself did not offer to recycle (the sink vetoed).
    ShellDeclined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecycleVerdict {
    /// Every check passed.
    Likely,
    /// Nothing blocked, but some evidence could not be read.
    Unverified,
    Blocked(RecycleBlock),
}

/// Windows' default Recycle Bin maximum for a volume of `total` bytes.
pub fn default_capacity_bytes(total: u64) -> u64 {
    let first = total.min(DEFAULT_FIRST_TIER);
    first / 10 + (total - first) / 20
}

/// Decides whether to ask the Shell to recycle a file of `size` logical bytes.
/// The first matching rule wins, which fixes the message and the tests.
pub fn recycle_verdict(facts: &RecycleFacts, size: u64) -> RecycleVerdict {
    if facts.drive_type != DRIVE_FIXED {
        return RecycleVerdict::Blocked(RecycleBlock::NoBinOnDrive);
    }
    if facts.policy_no_recycle.nonzero() {
        return RecycleVerdict::Blocked(RecycleBlock::DisabledByPolicy);
    }
    if facts.volume_nuke_on_delete.nonzero() {
        return RecycleVerdict::Blocked(RecycleBlock::DeletesImmediately);
    }
    // Without the volume GUID an explicit maximum cannot be read, so the
    // default must not be applied: it could be lower than the real limit.
    let capacity = if facts.volume_resolved {
        match facts.volume_max_capacity_mb {
            Setting::Value(mb) => Some(u64::from(mb).saturating_mul(MIB)),
            // Windows records the default as whole MiB, so a file between the
            // recorded limit and the unrounded estimate is already too large.
            Setting::Absent => facts
                .volume_total_bytes
                .filter(|total| *total > 0)
                .map(|total| default_capacity_bytes(total) / MIB * MIB),
            Setting::Unreadable => None,
        }
    } else {
        None
    };
    // The Shell recycles a file exactly at the maximum and deletes one byte
    // over; refusing at the boundary costs nothing and fails safe.
    if let Some(limit) = capacity
        && size >= limit
    {
        return RecycleVerdict::Blocked(RecycleBlock::TooLarge { size, limit });
    }
    if facts.policy_no_recycle.unreadable()
        || facts.volume_nuke_on_delete.unreadable()
        || facts.volume_max_capacity_mb.unreadable()
        || !facts.volume_resolved
        || capacity.is_none()
    {
        RecycleVerdict::Unverified
    } else {
        RecycleVerdict::Likely
    }
}

/// Extracts `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` from a volume name such as
/// `\\?\Volume{...}\`, the key name the Bin settings are stored under.
pub fn volume_guid_braced(volume_name: &str) -> Option<&str> {
    let rest = volume_name.strip_prefix(r"\\?\Volume")?;
    let braced = rest.strip_suffix('\\').unwrap_or(rest);
    let inner = braced.strip_prefix('{')?.strip_suffix('}')?;
    let groups: Vec<&str> = inner.split('-').collect();
    let shape = [8, 4, 4, 4, 12];
    let valid = groups.len() == shape.len()
        && groups
            .iter()
            .zip(shape)
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()));
    valid.then_some(braced)
}

/// Binary-unit size text, matching the rest of the interface.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

impl RecycleBlock {
    /// A plain sentence for the user. None claims the file was already deleted.
    pub fn reason(&self) -> String {
        match *self {
            Self::NoBinOnDrive => "Windows normally keeps no Recycle Bin on removable drives such as USB flash drives and memory cards, so it would delete this file permanently.".into(),
            Self::DisabledByPolicy => "A Windows policy turns the Recycle Bin off, so Windows would delete this file permanently.".into(),
            Self::DeletesImmediately => "This drive is set to remove files immediately instead of using the Recycle Bin, so Windows would delete this file permanently.".into(),
            Self::TooLarge { limit: 0, .. } => "This drive's Recycle Bin has a maximum size of 0, so Windows would delete this file permanently.".into(),
            Self::TooLarge { size, limit } => format!(
                "This file ({}) is too large for this drive's Recycle Bin (limit about {}), so Windows would delete it permanently.",
                format_size(size),
                format_size(limit)
            ),
            Self::ShellDeclined => "Windows did not offer the Recycle Bin for this file (the drive may have no Recycle Bin, or Windows is set to delete files immediately), so it would delete the file permanently.".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh Windows profile: fixed disk, every Bin setting at its default.
    fn ordinary_c_drive() -> RecycleFacts {
        RecycleFacts {
            drive_type: DRIVE_FIXED,
            policy_no_recycle: Setting::Absent,
            volume_resolved: true,
            volume_nuke_on_delete: Setting::Absent,
            volume_max_capacity_mb: Setting::Absent,
            volume_total_bytes: Some(348_915_757_056),
        }
    }

    #[test]
    fn default_capacity_matches_the_observed_windows_value() {
        // Windows wrote MaxCapacity=18685 MiB for this volume size.
        assert_eq!(default_capacity_bytes(348_915_757_056) >> 20, 18685);
        assert_eq!(default_capacity_bytes(40 << 30), 4 << 30);
        assert_eq!(default_capacity_bytes(100 << 30), 7_516_192_768);
        assert_eq!(default_capacity_bytes(0), 0);
        let _ = default_capacity_bytes(u64::MAX);
    }

    #[test]
    fn ordinary_fixed_drive_with_default_settings_is_likely() {
        // Regression guard: the everyday case must never be refused.
        let facts = ordinary_c_drive();
        for size in [0, 100 * MIB, 1 << 30, 18685 * MIB - 1] {
            assert_eq!(
                recycle_verdict(&facts, size),
                RecycleVerdict::Likely,
                "{size}"
            );
        }
        // Windows recorded 18685 MiB for this volume: the first size it refuses.
        assert_eq!(
            recycle_verdict(&facts, 18685 * MIB),
            RecycleVerdict::Blocked(RecycleBlock::TooLarge {
                size: 18685 * MIB,
                limit: 18685 * MIB,
            })
        );
    }

    #[test]
    fn explicit_capacity_overrides_the_default_in_both_directions() {
        let mut facts = ordinary_c_drive();
        facts.volume_max_capacity_mb = Setting::Value(18685);
        assert_eq!(recycle_verdict(&facts, 1 << 30), RecycleVerdict::Likely);
        facts.volume_max_capacity_mb = Setting::Value(1);
        assert_eq!(recycle_verdict(&facts, MIB - 1), RecycleVerdict::Likely);
        for size in [MIB, MIB + 1] {
            assert_eq!(
                recycle_verdict(&facts, size),
                RecycleVerdict::Blocked(RecycleBlock::TooLarge { size, limit: MIB })
            );
        }
        // An explicit small maximum beats a large default.
        facts.volume_total_bytes = Some(100 << 30);
        assert!(matches!(
            recycle_verdict(&facts, 2 * MIB),
            RecycleVerdict::Blocked(RecycleBlock::TooLarge { .. })
        ));
        // Absent: the default applies (10 MiB for a 100 MiB volume).
        facts.volume_max_capacity_mb = Setting::Absent;
        facts.volume_total_bytes = Some(100 * MIB);
        assert!(matches!(
            recycle_verdict(&facts, 20 * MIB),
            RecycleVerdict::Blocked(RecycleBlock::TooLarge { .. })
        ));
        assert_eq!(recycle_verdict(&facts, MIB), RecycleVerdict::Likely);
    }

    #[test]
    fn zero_capacity_blocks_even_an_empty_file() {
        let mut facts = ordinary_c_drive();
        facts.volume_max_capacity_mb = Setting::Value(0);
        assert_eq!(
            recycle_verdict(&facts, 0),
            RecycleVerdict::Blocked(RecycleBlock::TooLarge { size: 0, limit: 0 })
        );
    }

    #[test]
    fn unknown_facts_never_block_a_fixed_drive() {
        let mut facts = ordinary_c_drive();
        facts.volume_total_bytes = None;
        for size in [0, 1 << 30, u64::MAX] {
            assert_eq!(recycle_verdict(&facts, size), RecycleVerdict::Unverified);
        }
        // A reported total of zero is no evidence either.
        facts.volume_total_bytes = Some(0);
        assert_eq!(recycle_verdict(&facts, 1 << 30), RecycleVerdict::Unverified);
        let mut facts = ordinary_c_drive();
        facts.volume_max_capacity_mb = Setting::Unreadable;
        assert_eq!(
            recycle_verdict(&facts, u64::MAX),
            RecycleVerdict::Unverified
        );
        let mut facts = ordinary_c_drive();
        facts.policy_no_recycle = Setting::Unreadable;
        assert_eq!(recycle_verdict(&facts, MIB), RecycleVerdict::Unverified);
        let mut facts = ordinary_c_drive();
        facts.volume_nuke_on_delete = Setting::Unreadable;
        assert_eq!(recycle_verdict(&facts, MIB), RecycleVerdict::Unverified);
        // Without the volume GUID the default must not be applied: an explicit
        // maximum could be higher than it.
        let mut facts = ordinary_c_drive();
        facts.volume_resolved = false;
        for size in [MIB, 1 << 40] {
            assert_eq!(recycle_verdict(&facts, size), RecycleVerdict::Unverified);
        }
    }

    #[test]
    fn only_fixed_drives_pass() {
        let mut facts = ordinary_c_drive();
        assert_eq!(recycle_verdict(&facts, MIB), RecycleVerdict::Likely);
        for drive_type in [0, 1, 2, 4, 5, 6, 99] {
            facts.drive_type = drive_type;
            assert_eq!(
                recycle_verdict(&facts, MIB),
                RecycleVerdict::Blocked(RecycleBlock::NoBinOnDrive),
                "{drive_type}"
            );
        }
    }

    #[test]
    fn policy_and_nuke_block_in_documented_order() {
        let mut facts = ordinary_c_drive();
        facts.policy_no_recycle = Setting::Value(1);
        assert_eq!(
            recycle_verdict(&facts, MIB),
            RecycleVerdict::Blocked(RecycleBlock::DisabledByPolicy)
        );
        facts.policy_no_recycle = Setting::Absent;
        facts.volume_nuke_on_delete = Setting::Value(1);
        assert_eq!(
            recycle_verdict(&facts, MIB),
            RecycleVerdict::Blocked(RecycleBlock::DeletesImmediately)
        );
        facts.policy_no_recycle = Setting::Value(1);
        assert_eq!(
            recycle_verdict(&facts, MIB),
            RecycleVerdict::Blocked(RecycleBlock::DisabledByPolicy)
        );
        facts.drive_type = 2;
        assert_eq!(
            recycle_verdict(&facts, MIB),
            RecycleVerdict::Blocked(RecycleBlock::NoBinOnDrive)
        );
        let mut zeroes = ordinary_c_drive();
        zeroes.policy_no_recycle = Setting::Value(0);
        zeroes.volume_nuke_on_delete = Setting::Value(0);
        assert_eq!(recycle_verdict(&zeroes, MIB), RecycleVerdict::Likely);
    }

    #[test]
    fn setting_combine_is_stricter_wins() {
        let all = [
            Setting::Absent,
            Setting::Value(0),
            Setting::Value(1),
            Setting::Unreadable,
        ];
        // Compare by meaning: two different nonzero values are equally strict.
        let kind = |setting: Setting| match setting {
            Setting::Absent => 0,
            Setting::Value(0) => 1,
            Setting::Unreadable => 2,
            Setting::Value(_) => 3,
        };
        for a in all {
            for b in all {
                let combined = a.combine(b);
                assert_eq!(kind(combined), kind(b.combine(a)), "{a:?} {b:?}");
                assert_eq!(
                    combined.nonzero(),
                    a.nonzero() || b.nonzero(),
                    "{a:?} {b:?}"
                );
                assert_eq!(kind(combined), kind(a).max(kind(b)), "{a:?} {b:?}");
            }
        }
        assert_eq!(Setting::Absent.combine(Setting::Absent), Setting::Absent);
        assert_eq!(
            Setting::Value(0).combine(Setting::Absent),
            Setting::Value(0)
        );
    }

    #[test]
    fn volume_guid_braced_accepts_only_volume_guid_paths() {
        let guid = "{01234567-89ab-CDEF-0123-456789abcdef}";
        assert_eq!(
            volume_guid_braced(&format!(r"\\?\Volume{guid}\")),
            Some(guid)
        );
        assert_eq!(
            volume_guid_braced(&format!(r"\\?\Volume{guid}")),
            Some(guid)
        );
        for bad in [
            r"C:\",
            "",
            r"\\?\Volume{}\",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcde}\",
            r"\\?\Volume{0123456g-89ab-cdef-0123-456789abcdef}\",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\extra",
            r"\\?\Volume01234567-89ab-cdef-0123-456789abcdef\",
        ] {
            assert_eq!(volume_guid_braced(bad), None, "{bad}");
        }
    }

    #[test]
    fn block_reasons_are_plain_and_truthful() {
        let blocks = [
            RecycleBlock::NoBinOnDrive,
            RecycleBlock::DisabledByPolicy,
            RecycleBlock::DeletesImmediately,
            RecycleBlock::TooLarge {
                size: 5 * MIB,
                limit: MIB,
            },
            RecycleBlock::TooLarge { size: 0, limit: 0 },
            RecycleBlock::ShellDeclined,
        ];
        for block in blocks {
            let reason = block.reason();
            assert!(reason.ends_with('.'), "{reason}");
            assert!(!reason.contains("fallback"), "{reason}");
            assert!(!reason.contains("was deleted"), "{reason}");
        }
        let large = RecycleBlock::TooLarge {
            size: 5 * MIB,
            limit: MIB,
        }
        .reason();
        assert!(
            large.contains("5.0 MiB") && large.contains("1.0 MiB"),
            "{large}"
        );
        let zero = RecycleBlock::TooLarge { size: 0, limit: 0 }.reason();
        assert!(zero.contains("maximum size of 0"), "{zero}");
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(3 * MIB / 2), "1.5 MiB");
    }
}
