#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Starting,
    Ready,
    Listening,
    Finalizing,
    Injecting,
    Error,
    ShuttingDown,
}

impl AppState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Ready => "Ready",
            Self::Listening => "Listening",
            Self::Finalizing => "Finalizing",
            Self::Injecting => "Injecting",
            Self::Error => "Error",
            Self::ShuttingDown => "Shutting down",
        }
    }
}

#[derive(Debug)]
pub struct SessionState {
    pub app_state: AppState,
    pub active_session_id: Option<u64>,
    next_session_id: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            app_state: AppState::Starting,
            active_session_id: None,
            next_session_id: 1,
        }
    }
}

impl SessionState {
    pub fn mark_ready(&mut self) {
        self.app_state = AppState::Ready;
    }

    pub fn mark_error(&mut self) {
        self.active_session_id = None;
        self.app_state = AppState::Error;
    }

    pub fn begin_model_loading(&mut self) -> bool {
        if self.app_state != AppState::Ready && self.app_state != AppState::Error {
            return false;
        }
        self.app_state = AppState::Starting;
        true
    }

    pub fn start_session(&mut self) -> Option<u64> {
        if self.app_state != AppState::Ready {
            return None;
        }

        let session_id = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        self.active_session_id = Some(session_id);
        self.app_state = AppState::Listening;
        Some(session_id)
    }

    pub fn begin_finalizing(&mut self, session_id: u64) -> bool {
        if self.app_state != AppState::Listening {
            return false;
        }

        if self.active_session_id != Some(session_id) {
            return false;
        }

        self.app_state = AppState::Finalizing;
        true
    }

    pub fn accepts(&self, session_id: u64) -> bool {
        self.active_session_id == Some(session_id)
    }

    pub fn begin_injecting(&mut self, session_id: u64) -> bool {
        if self.app_state != AppState::Finalizing {
            return false;
        }
        if !self.accepts(session_id) {
            return false;
        }
        self.app_state = AppState::Injecting;
        true
    }

    pub fn complete_session(&mut self, session_id: u64) -> bool {
        if !self.accepts(session_id) {
            return false;
        }

        self.active_session_id = None;
        self.app_state = AppState::Ready;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_session_cannot_complete_current_session() {
        let mut state = SessionState::default();
        state.mark_ready();

        let first = state.start_session().unwrap();
        assert!(state.begin_finalizing(first));
        assert!(state.complete_session(first));

        let second = state.start_session().unwrap();
        assert!(!state.complete_session(first));
        assert_eq!(state.active_session_id, Some(second));
    }
}
