use std::process::Child;

/// Recording-scoped macOS idle-sleep protection.
///
/// `/usr/bin/caffeinate -i` creates the same class of assertion as an
/// IOPMAssertion that prevents *idle system sleep*. It deliberately does not
/// use `-d`, so the display may turn off and the user may lock the screen. A
/// closed lid or explicit Sleep command can still suspend USB, which the
/// recording recovery state machine handles separately.
pub(crate) struct IdleSleepAssertion {
    child: Option<Child>,
    active: bool,
}

impl IdleSleepAssertion {
    pub(crate) fn acquire() -> Result<Self, String> {
        #[cfg(all(target_os = "macos", not(test)))]
        {
            let child = std::process::Command::new("/usr/bin/caffeinate")
                .arg("-i")
                .spawn()
                .map_err(|error| format!("无法建立 macOS 防空闲睡眠保护：{error}"))?;
            Ok(Self {
                child: Some(child),
                active: true,
            })
        }

        #[cfg(any(not(target_os = "macos"), test))]
        Ok(Self {
            child: None,
            active: cfg!(test),
        })
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn release(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.active = false;
    }
}

impl Drop for IdleSleepAssertion {
    fn drop(&mut self) {
        self.release();
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
    }
}
