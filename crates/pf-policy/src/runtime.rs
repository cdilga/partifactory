//! The role x runtime matrix (§3a "What each role can do at runtime"), enforced in `/_rt` and in
//! every op. One pure function, table-tested row by row.

use crate::{COMMENT, EDIT};

/// Who is calling the runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtSubject {
    /// A public-link viewer on a **Can view** link.
    LinkViewer,
    /// A public-link guest on a **Can contribute** link.
    LinkGuest,
    /// A signed-in person (or an agent) with this effective role.
    Role(u8),
}

/// What they are trying to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtOp {
    /// Personal state (the `localStorage` shim, personal `storage`), server-side.
    PersonalState,
    SharedRead,
    SharedWrite,
    Comment,
    SelfPublish,
    SourceEdit,
    AssetUpload,
    Backend,
}

/// `parts.shared_data_writers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SharedDataWriters {
    Editors,
    #[default]
    Viewers,
}

impl SharedDataWriters {
    pub fn parse(s: &str) -> Self {
        if s == "editors" {
            Self::Editors
        } else {
            Self::Viewers
        }
    }
}

/// The per-part switches the matrix reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PartFlags {
    pub shared_data_writers: SharedDataWriters,
    /// `parts.link_comments`: public-link viewers may comment (opt-in).
    pub link_comments: bool,
}

/// Whether `subject` may do `op` on a part with `flags`.
pub fn allows(subject: RtSubject, op: RtOp, flags: PartFlags) -> bool {
    use RtOp::*;
    match subject {
        // Public-link viewers: memory-only personal state, read shared data, comments only if
        // opted in, backend calls as `role: public`.
        RtSubject::LinkViewer => match op {
            SharedRead | Backend => true,
            Comment => flags.link_comments,
            _ => false,
        },
        // Guests on a Can-contribute link: read and write shared data (rate-limited and capped
        // elsewhere), backend as `role: guest`, nothing else.
        RtSubject::LinkGuest => matches!(op, SharedRead | SharedWrite | Backend),
        RtSubject::Role(r) => match op {
            PersonalState | SharedRead | Backend => true,
            SharedWrite => r >= EDIT || flags.shared_data_writers == SharedDataWriters::Viewers,
            Comment | AssetUpload => r >= COMMENT,
            SelfPublish | SourceEdit => r >= EDIT,
        },
    }
}

/// The role label a backend sees (§3f).
pub fn backend_role(subject: RtSubject) -> &'static str {
    match subject {
        RtSubject::LinkViewer => "public",
        RtSubject::LinkGuest => "guest",
        RtSubject::Role(r) => crate::role_name(r),
    }
}
