#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PermitState {
    credit: bool,
    active: bool,
    faulted: bool,
}

impl PermitState {
    pub(crate) fn needs_wait(self) -> bool {
        !self.credit && !self.active && !self.faulted
    }

    pub(crate) fn is_faulted(self) -> bool {
        self.faulted
    }

    pub(crate) fn ready(&mut self) {
        self.credit = true;
    }

    pub(crate) fn begin(&mut self, modal: bool) -> bool {
        if self.active || self.faulted || (!self.credit && !modal) {
            return false;
        }
        self.active = true;
        true
    }

    pub(crate) fn presented(&mut self) {
        self.credit = false;
    }

    pub(crate) fn failed(&mut self) {
        self.credit = false;
        self.faulted = true;
    }

    pub(crate) fn finish(&mut self) -> bool {
        self.active = false;
        // No Present means no queue entry. Keep readiness, but never keep
        // an active frame permit after its callback returns.
        self.credit && !self.faulted
    }

    pub(crate) fn resized(&mut self) {
        self.credit = false;
    }
}

pub(crate) fn enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

pub(crate) fn wait_millis(refresh_hz: u32) -> u32 {
    // Unknown display timing is a nonblocking poll. Round down so a wait
    // never exceeds the reported display period.
    if refresh_hz > 1 { 1000 / refresh_hz } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_frame_can_start_before_readiness() {
        let mut state = PermitState::default();
        assert!(state.needs_wait());
        assert!(!state.begin(false));
        state.ready();
        assert!(state.begin(false));
    }

    #[test]
    fn nested_frames_cannot_share_a_permit() {
        let mut state = PermitState::default();
        state.ready();
        assert!(state.begin(false));
        assert!(!state.begin(false));
        assert!(!state.begin(true));
        state.presented();
        assert!(!state.begin(false));
        state.finish();
        assert!(state.needs_wait());
    }

    #[test]
    fn unchanged_or_skipped_frames_return_credit_without_an_active_permit() {
        let mut state = PermitState::default();
        state.ready();
        for _ in 0..3 {
            assert!(state.begin(false));
            assert!(state.finish());
        }
        assert!(state.begin(false));
        state.presented();
        assert!(!state.finish());
        assert!(!state.begin(false));
        assert!(state.needs_wait());
    }

    #[test]
    fn draw_or_present_failure_cannot_leave_a_permissive_gate() {
        let mut state = PermitState::default();
        state.ready();
        assert!(state.begin(false));
        state.failed();
        assert!(!state.finish());
        state.ready();
        assert!(!state.begin(false));
        assert!(!state.begin(true));
        assert!(!state.needs_wait());
        let recovered = PermitState::default();
        assert!(recovered.needs_wait());
    }

    #[test]
    fn resize_discards_readiness_from_old_buffers() {
        let mut state = PermitState::default();
        state.ready();
        state.resized();
        assert!(!state.begin(false));
        assert!(state.needs_wait());
    }

    #[test]
    fn modal_bypass_does_not_leave_a_permit_after_exit() {
        let mut state = PermitState::default();
        assert!(state.begin(true));
        state.presented();
        state.finish();
        assert!(!state.begin(false));
        assert!(state.needs_wait());
    }

    #[test]
    fn only_explicit_one_enables_waitable_flags() {
        for value in [None, Some(""), Some("0"), Some("true"), Some(" 1")] {
            assert!(!enabled(value));
        }
        assert!(enabled(Some("1")));
    }

    #[test]
    fn timeout_tracks_refresh_and_unknown_timing_never_blocks() {
        assert_eq!(wait_millis(60), 16);
        assert_eq!(wait_millis(120), 8);
        assert_eq!(wait_millis(175), 5);
        assert_eq!(wait_millis(240), 4);
        assert_eq!(wait_millis(0), 0);
        assert_eq!(wait_millis(1), 0);
    }
}
