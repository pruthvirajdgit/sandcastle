use sandcastle_runtime::{Language, IsolationLevel, SandboxId};
use std::time::Instant;

/// Tracks a live sandbox session.
pub struct Session {
    pub sandbox_id: SandboxId,
    pub language: Language,
    pub isolation: IsolationLevel,
    pub created_at: Instant,
    pub last_active: Instant,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Active,
    Expired,
    Destroying,
}

impl Session {
    pub fn new(sandbox_id: SandboxId, language: Language, isolation: IsolationLevel) -> Self {
        let now = Instant::now();
        Self {
            sandbox_id,
            language,
            isolation,
            created_at: now,
            last_active: now,
            status: SessionStatus::Active,
        }
    }

    pub fn touch(&mut self) {
        self.last_active = Instant::now();
    }
}
