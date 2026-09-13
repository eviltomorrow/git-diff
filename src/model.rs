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