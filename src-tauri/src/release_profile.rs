//! Compile-time product profile shared by the Rust backend.
//!
//! `NOTED_RELEASE_PROFILE=alpha` produces the focused Mac alpha. Development
//! builds keep every workshop capability available. Runtime checks remain in
//! command handlers so a stale settings file cannot re-enable deferred work.

pub fn name() -> &'static str {
    if option_env!("NOTED_RELEASE_PROFILE") == Some("alpha") {
        "alpha"
    } else {
        "development"
    }
}

pub fn is_alpha() -> bool {
    name() == "alpha"
}

pub fn meeting_recording() -> bool {
    true
}

pub fn system_audio() -> bool {
    true
}

pub fn themes() -> bool {
    true
}

pub fn provider_matrix() -> bool {
    true
}

pub fn google_calendar() -> bool {
    true
}

pub fn balanced_inference() -> bool {
    !is_alpha()
}

pub fn noted_hosted() -> bool {
    !is_alpha()
}

pub fn phone_lan() -> bool {
    !is_alpha()
}

pub fn diarization() -> bool {
    !is_alpha()
}

pub fn video_capture() -> bool {
    !is_alpha()
}

pub fn billing() -> bool {
    !is_alpha()
}

pub fn disabled(feature: &str) -> String {
    format!("{feature} is not included in the {} release profile", name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_alpha_capabilities_are_always_enabled() {
        assert!(meeting_recording());
        assert!(system_audio());
        assert!(themes());
        assert!(provider_matrix());
        assert!(google_calendar());
    }

    #[test]
    fn alpha_defers_non_core_capabilities() {
        if is_alpha() {
            assert!(!balanced_inference());
            assert!(!noted_hosted());
            assert!(!phone_lan());
            assert!(!diarization());
            assert!(!video_capture());
            assert!(!billing());
        }
    }
}
