//! macOS process activities that keep a running measurement from being
//! throttled or put to sleep.
//!
//! Two independent scopes exist:
//!
//! * [`IdleSleepAssertion`] is recording-scoped and controlled by the user's
//!   sleep-protection preference. It is the in-process equivalent of
//!   `caffeinate -i`: idle *system* sleep is prevented, but the display may
//!   turn off and the user may lock the screen. A closed lid or an explicit
//!   Sleep command can still suspend USB, which the recording recovery state
//!   machine handles separately.
//! * [`StreamingActivity`] is held while the device streams. It only opts the
//!   process out of App Nap, so timer coalescing and priority demotion cannot
//!   stall the USB polling loop while the window is hidden or occluded. It
//!   never prevents sleep.
//!
//! Both are `-[NSProcessInfo beginActivityWithOptions:reason:]` activities.
//! The system ends them together with the process, so a crash or a force quit
//! cannot leave a stray `caffeinate` process holding the Mac awake.

#[cfg(target_os = "macos")]
mod platform {
    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    const IDLE_SLEEP_REASON: &str = "KM003C Workbench is recording measurements";
    const STREAMING_REASON: &str = "KM003C Workbench is streaming measurements";
    pub(super) const IDLE_SLEEP_OPTIONS: NSActivityOptions = NSActivityOptions::UserInitiated;
    pub(super) const STREAMING_OPTIONS: NSActivityOptions = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep;

    /// An activity token that ends exactly once, when dropped.
    pub(super) struct Activity(Retained<ProtocolObject<dyn NSObjectProtocol>>);

    impl Activity {
        pub(super) fn begin(options: NSActivityOptions, reason: &str) -> Self {
            let reason = NSString::from_str(reason);
            Self(NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &reason))
        }
    }

    impl Drop for Activity {
        fn drop(&mut self) {
            // SAFETY: the token was returned by `beginActivityWithOptions:reason:`
            // on the process-wide `NSProcessInfo` and is ended exactly once.
            unsafe { NSProcessInfo::processInfo().endActivity(&self.0) };
        }
    }

    /// Prevents idle system sleep and App Nap.
    pub(super) fn idle_sleep() -> Activity {
        Activity::begin(IDLE_SLEEP_OPTIONS, IDLE_SLEEP_REASON)
    }

    /// Prevents App Nap only; the system may still idle-sleep.
    pub(super) fn streaming() -> Activity {
        Activity::begin(STREAMING_OPTIONS, STREAMING_REASON)
    }
}

/// Recording-scoped idle-sleep protection.
pub(crate) struct IdleSleepAssertion {
    #[cfg(target_os = "macos")]
    activity: Option<platform::Activity>,
    active: bool,
}

impl IdleSleepAssertion {
    /// Begins the protection. The `Result` is kept for callers that report an
    /// unavailable protection; the in-process activity itself cannot fail.
    pub(crate) fn acquire() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                activity: Some(platform::idle_sleep()),
                active: true,
            })
        }

        #[cfg(not(target_os = "macos"))]
        Ok(Self { active: cfg!(test) })
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn release(&mut self) {
        #[cfg(target_os = "macos")]
        {
            self.activity = None;
        }
        self.active = false;
    }
}

/// Opts the process out of App Nap while the device streams.
pub(crate) struct StreamingActivity {
    #[cfg(target_os = "macos")]
    _activity: platform::Activity,
}

impl StreamingActivity {
    pub(crate) fn begin() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            _activity: platform::streaming(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assertion_releases_deterministically() {
        let mut assertion = IdleSleepAssertion::acquire().unwrap();
        assert!(assertion.is_active());
        assertion.release();
        assert!(!assertion.is_active());
        // A second release is harmless and must not end the activity twice.
        assertion.release();
        assert!(!assertion.is_active());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn streaming_activity_can_be_held_and_dropped() {
        let first = StreamingActivity::begin();
        let second = StreamingActivity::begin();
        drop(first);
        drop(second);
    }

    /// A reason no other test in this process uses. Tests run in parallel and
    /// recording tests hold the production activities concurrently.
    #[cfg(target_os = "macos")]
    fn unique_reason(label: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        format!(
            "km003c-test-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// The `pmset -g assertions` lines that mention `reason`.
    #[cfg(target_os = "macos")]
    fn power_assertions_named(reason: &str) -> Vec<String> {
        let output = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "assertions"])
            .output()
            .expect("pmset is part of every macOS installation");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(reason))
            .map(str::to_owned)
            .collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recording_activity_holds_an_idle_sleep_assertion_until_dropped() {
        let reason = unique_reason("idle");
        let activity = platform::Activity::begin(platform::IDLE_SLEEP_OPTIONS, &reason);
        let held = power_assertions_named(&reason);
        let owner = format!("pid {}(", std::process::id());
        assert!(
            held.iter()
                .any(|line| line.contains(&owner) && line.contains("PreventUserIdleSystemSleep")),
            "expected an idle-sleep assertion owned by this process, got: {held:?}"
        );
        drop(activity);
        let released = power_assertions_named(&reason);
        assert!(released.is_empty(), "the assertion must end on drop, got: {released:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn streaming_activity_never_prevents_idle_sleep() {
        let reason = unique_reason("streaming");
        let activity = platform::Activity::begin(platform::STREAMING_OPTIONS, &reason);
        let held = power_assertions_named(&reason);
        assert!(
            !held.iter().any(|line| line.contains("PreventUserIdleSystemSleep")),
            "streaming must not hold an idle-sleep assertion, got: {held:?}"
        );
        drop(activity);
    }
}
