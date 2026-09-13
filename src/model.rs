use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

impl Status {
    pub fn letter(self) -> &'static str {
        match self {
            Status::Modified => "M",
            Status::Added => "A",
            Status::Deleted => "D",
            Status::Renamed => "R",
            Status::Untracked => "U",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonMode {
    WorkingVsHead,
    StagedVsHead,
    StagedVsWorking,
    CommitVsHead,
}

impl ComparisonMode {
    pub fn label(self) -> &'static str {
        match self {
            ComparisonMode::WorkingVsHead => "工作区↔HEAD",
            ComparisonMode::StagedVsHead => "暂存区↔HEAD",
            ComparisonMode::StagedVsWorking => "暂存区↔工作区",
            ComparisonMode::CommitVsHead => "commit↔HEAD",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub status: Status,
    pub path: String,
    pub old_path: Option<String>,
    pub added: u64,
    pub deleted: u64,
    pub is_binary: bool,
}

impl fmt::Display for ChangedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.status.letter(), self.path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitEntry {
    pub short_hash: String,
    pub title: String,
    pub date: String,
    pub author: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSides {
    pub original: Option<Vec<u8>>,
    pub changed: Option<Vec<u8>>,
}

/// One end of a comparison: where a side's content comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideRef {
    Worktree,
    Index,
    Head,
    Commit(String),
}

impl SideRef {
    /// The label shown in the diff pane header for this end.
    pub fn label(&self) -> String {
        match self {
            SideRef::Worktree => "工作区".to_string(),
            SideRef::Index => "暂存区".to_string(),
            SideRef::Head => "HEAD".to_string(),
            SideRef::Commit(c) => c.clone(),
        }
    }
}

/// The ref pairing for a comparison mode: which end is the 原始侧 and which is
/// the 变更侧. Both consumers (git data fetch, pane headers, diff loading) ask
/// this one module instead of re-deriving the mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidePair {
    pub original: SideRef,
    pub changed: SideRef,
}

impl SidePair {
    /// The pairing for a mode. `commit` supplies the 变更侧 ref for mode D.
    pub fn for_mode(mode: ComparisonMode, commit: Option<&str>) -> SidePair {
        match mode {
            ComparisonMode::WorkingVsHead => SidePair {
                original: SideRef::Head,
                changed: SideRef::Worktree,
            },
            ComparisonMode::StagedVsHead => SidePair {
                original: SideRef::Head,
                changed: SideRef::Index,
            },
            ComparisonMode::StagedVsWorking => SidePair {
                original: SideRef::Index,
                changed: SideRef::Worktree,
            },
            ComparisonMode::CommitVsHead => SidePair {
                original: SideRef::Commit(commit.unwrap_or("commit").to_string()),
                changed: SideRef::Head,
            },
        }
    }
}
