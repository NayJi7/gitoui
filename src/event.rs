use std::{
    fmt::{self, Debug, Formatter},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
};

use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use serde::{
    de::{self, Deserializer, Visitor},
    Deserialize,
};

use crate::view::RefreshViewContext;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(usize, usize),
    Quit,
    OpenDetail,
    OpenUncommitted,
    CloseDetail,
    OpenUserCommand(usize),
    CloseUserCommand,
    OpenRefs,
    CloseRefs,
    OpenHelp,
    CloseHelp,
    OpenConfig,
    CloseConfig,
    OpenFileDiff { hash: String, file_path: String },
    CloseDiff,
    CloseDiffToDetail,
    SelectNewerCommit,
    SelectOlderCommit,
    SelectParentCommit,
    CopyToClipboard { name: String, value: String },
    Refresh(RefreshViewContext),
    ClearStatusLine,
    UpdateStatusInput(String, Option<u16>, Option<String>),
    NotifyInfo(String),
    NotifySuccess(String),
    NotifyWarn(String),
    NotifyError(String),
    // Phase 2 - Git Actions
    OpenDialog(DialogKind),
    CloseDialog,
    DialogConfirm,
    DialogCancel,
    DialogInput(String),
    ExecuteGitAction { target: String, action: GitAction },
    OpenBranchDetail { branch_name: String },
    OpenTagDetail { tag_name: String },
    StageFile { file: String },
    UnstageFile { file: String },
    DiscardFile { file: String },
}

#[derive(Debug, Clone)]
pub enum DialogKind {
    // Commit actions
    AddTag { target: String },
    CreateBranch { target: String },
    Checkout { target: String, is_branch: bool },
    CherryPick { target: String },
    Revert { target: String },
    Drop { target: String },
    Merge { target: String, is_branch: bool },
    Rebase { target: String },
    Reset { target: String },
    // Branch actions
    RenameBranch { branch: String },
    DeleteBranch { branch: String, is_remote: bool },
    PushBranch { branch: String },
    PullBranch { branch: String },
    // Tag actions
    DeleteTag { tag: String },
    PushTag { tag: String },
    // Stash actions
    CreateBranchFromStash { target: String, stash_ref: String },
    // Uncommitted actions
    StashWithMessage,
    CommitWithMessage,
    CleanUntracked,
    // Confirmations
    ConfirmDiscardFile { file: String },
    ConfirmDiscardAll,
    ConfirmStageAll,
    ConfirmUnstageAll,
    ConfirmPopStash { stash_ref: String },
    ConfirmDropStash { stash_ref: String },
}

#[derive(Debug, Clone)]
pub enum GitAction {
    // Commit actions
    Checkout,
    CreateBranch { name: String, checkout: bool },
    AddTag { name: String, annotated: bool, message: Option<String> },
    CherryPick { no_commit: bool, record_origin: bool },
    Revert,
    Drop,
    Merge { no_ff: bool, squash: bool, no_commit: bool },
    Rebase { ignore_date: bool, interactive: bool },
    Reset { mode: String },
    // Branch actions
    DeleteBranch { force: bool },
    RenameBranch { new_name: String },
    PushBranch { force: bool },
    PullBranch { rebase: bool },
    Fetch,
    CreateArchive,
    // Tag actions
    DeleteTag,
    PushTag,
    // Stash actions
    ApplyStash,
    PopStash,
    DropStash,
    CreateBranchFromStash { branch_name: String },
    // Uncommitted actions
    StageFile { file: String },
    StageAll,
    UnstageFile { file: String },
    UnstageAll,
    DiscardFile { file: String },
    DiscardAll,
    Stash { message: Option<String> },
    Commit { message: String },
    CleanUntracked,
    Push,
}

#[derive(Clone)]
pub struct Sender {
    tx: mpsc::Sender<AppEvent>,
}

impl Sender {
    pub fn send(&self, event: AppEvent) {
        self.tx.send(event).unwrap();
    }
}

impl Debug for Sender {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Sender")
    }
}

pub struct Receiver {
    rx: mpsc::Receiver<AppEvent>,
}

impl Receiver {
    fn recv(&self) -> AppEvent {
        self.rx.recv().unwrap()
    }
}

impl Debug for Receiver {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Receiver")
    }
}

#[derive(Debug)]
pub struct EventController {
    tx: Sender,
    rx: Receiver,
    stop: Arc<AtomicBool>,
    handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl EventController {
    pub fn init() -> Self {
        let (tx, rx) = mpsc::channel();
        let tx = Sender { tx };
        let rx = Receiver { rx };

        let controller = EventController {
            tx: tx.clone(),
            rx,
            stop: Arc::new(AtomicBool::new(false)),
            handle: Arc::new(Mutex::new(None)),
        };
        controller.start();

        controller
    }

    pub fn start(&self) {
        self.stop.store(false, Ordering::Relaxed);
        let stop = self.stop.clone();
        let tx = self.tx.clone();
        let handle = thread::spawn(move || loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match ratatui::crossterm::event::poll(std::time::Duration::from_millis(100)) {
                Ok(true) => match ratatui::crossterm::event::read() {
                    Ok(e) => match e {
                        ratatui::crossterm::event::Event::Key(key) => {
                            tx.send(AppEvent::Key(key));
                        }
                        ratatui::crossterm::event::Event::Mouse(mouse) => {
                            tx.send(AppEvent::Mouse(mouse));
                        }
                        ratatui::crossterm::event::Event::Resize(w, h) => {
                            tx.send(AppEvent::Resize(w as usize, h as usize));
                        }
                        _ => {}
                    },
                    Err(e) => {
                        panic!("Failed to read event: {e}");
                    }
                },
                Ok(false) => {
                    continue;
                }
                Err(e) => {
                    panic!("Failed to poll event: {e}");
                }
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
    }

    pub fn resume(&self) {
        ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::EnterAlternateScreen
        )
        .unwrap();
        ratatui::crossterm::terminal::enable_raw_mode().unwrap();

        self.drain_crossterm_event();
        self.start();
    }

    pub fn suspend(&self) {
        self.stop();

        ratatui::crossterm::terminal::disable_raw_mode().unwrap();
        ratatui::crossterm::execute!(
            std::io::stdout(),
            ratatui::crossterm::terminal::LeaveAlternateScreen
        )
        .unwrap();
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }

    fn drain_crossterm_event(&self) {
        while let Ok(true) = ratatui::crossterm::event::poll(std::time::Duration::from_millis(0)) {
            let _ = ratatui::crossterm::event::read();
        }
    }

    pub fn sender(&self) -> Sender {
        self.tx.clone()
    }

    pub fn send(&self, event: AppEvent) {
        self.tx.send(event);
    }

    pub fn recv(&self) -> AppEvent {
        self.rx.recv()
    }
}

// The event triggered by user's key input
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserEvent {
    ForceQuit,
    Quit,
    HelpToggle,
    Cancel,
    Close,
    NavigateUp,
    NavigateDown,
    NavigateRight,
    NavigateLeft,
    SelectUp,
    SelectDown,
    GoToTop,
    GoToBottom,
    GoToParent,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    SelectTop,
    SelectMiddle,
    SelectBottom,
    GoToNext,
    GoToPrevious,
    Confirm,
    RefList,
    Search,
    UserCommand(usize),
    IgnoreCaseToggle,
    FuzzyToggle,
    Refresh,
    Config,
    ShortCopy,
    FullCopy,
    Unknown,
    // Phase 2 - Commit actions
    AddTag,
    CreateBranch,
    Checkout,
    CherryPick,
    Revert,
    Drop,
    Merge,
    Rebase,
    Reset,
    // Phase 2 - Uncommitted actions
    Stage,
    StageAll,
    Unstage,
    UnstageAll,
    Discard,
    DiscardAll,
    Stash,
    Commit,
    CleanUntracked,
    // Phase 2 - Branch actions
    RenameBranch,
    DeleteBranch,
    PushBranch,
    PullBranch,
    CreateArchive,
    UnselectBranch,
    CopyBranchName,
    // Phase 2 - Tag actions
    DeleteTag,
    PushTag,
    CopyTagName,
    // Phase 2 - Stash actions
    ApplyStash,
    PopStash,
    DropStash,
    CreateBranchFromStash,
    CopyStashName,
    CopyStashHash,
}

impl<'de> Deserialize<'de> for UserEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UserEventVisitor;

        impl<'de> Visitor<'de> for UserEventVisitor {
            type Value = UserEvent;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string representing a user event")
            }

            fn visit_str<E>(self, value: &str) -> Result<UserEvent, E>
            where
                E: de::Error,
            {
                if value.starts_with("user_command_") {
                    if let Some(num) = parse_user_command_number(value) {
                        Ok(UserEvent::UserCommand(num))
                    } else {
                        let msg = format!("Invalid user_command_n format: {value}",);
                        Err(de::Error::custom(msg))
                    }
                } else {
                    match value {
                        "force_quit" => Ok(UserEvent::ForceQuit),
                        "quit" => Ok(UserEvent::Quit),
                        "help_toggle" => Ok(UserEvent::HelpToggle),
                        "cancel" => Ok(UserEvent::Cancel),
                        "close" => Ok(UserEvent::Close),
                        "navigate_up" => Ok(UserEvent::NavigateUp),
                        "navigate_down" => Ok(UserEvent::NavigateDown),
                        "navigate_right" => Ok(UserEvent::NavigateRight),
                        "navigate_left" => Ok(UserEvent::NavigateLeft),
                        "select_up" => Ok(UserEvent::SelectUp),
                        "select_down" => Ok(UserEvent::SelectDown),
                        "go_to_top" => Ok(UserEvent::GoToTop),
                        "go_to_bottom" => Ok(UserEvent::GoToBottom),
                        "go_to_parent" => Ok(UserEvent::GoToParent),
                        "scroll_up" => Ok(UserEvent::ScrollUp),
                        "scroll_down" => Ok(UserEvent::ScrollDown),
                        "page_up" => Ok(UserEvent::PageUp),
                        "page_down" => Ok(UserEvent::PageDown),
                        "half_page_up" => Ok(UserEvent::HalfPageUp),
                        "half_page_down" => Ok(UserEvent::HalfPageDown),
                        "select_top" => Ok(UserEvent::SelectTop),
                        "select_middle" => Ok(UserEvent::SelectMiddle),
                        "select_bottom" => Ok(UserEvent::SelectBottom),
                        "go_to_next" => Ok(UserEvent::GoToNext),
                        "go_to_previous" => Ok(UserEvent::GoToPrevious),
                        "confirm" => Ok(UserEvent::Confirm),
                        "ref_list" | "ref_list_toggle" => Ok(UserEvent::RefList),
                        "search" => Ok(UserEvent::Search),
                        "ignore_case_toggle" => Ok(UserEvent::IgnoreCaseToggle),
                        "fuzzy_toggle" => Ok(UserEvent::FuzzyToggle),
                        "refresh" => Ok(UserEvent::Refresh),
                        "config" => Ok(UserEvent::Config),
                        "short_copy" => Ok(UserEvent::ShortCopy),
                        "full_copy" => Ok(UserEvent::FullCopy),
                        "add_tag" => Ok(UserEvent::AddTag),
                        "create_branch" => Ok(UserEvent::CreateBranch),
                        "checkout" => Ok(UserEvent::Checkout),
                        "cherry_pick" => Ok(UserEvent::CherryPick),
                        "revert" => Ok(UserEvent::Revert),
                        "drop" => Ok(UserEvent::Drop),
                        "merge" => Ok(UserEvent::Merge),
                        "rebase" => Ok(UserEvent::Rebase),
                        "reset" => Ok(UserEvent::Reset),
                        "stage" => Ok(UserEvent::Stage),
                        "stage_all" => Ok(UserEvent::StageAll),
                        "unstage" => Ok(UserEvent::Unstage),
                        "unstage_all" => Ok(UserEvent::UnstageAll),
                        "discard" => Ok(UserEvent::Discard),
                        "discard_all" => Ok(UserEvent::DiscardAll),
                        "stash" => Ok(UserEvent::Stash),
                        "commit" => Ok(UserEvent::Commit),
                        "clean_untracked" => Ok(UserEvent::CleanUntracked),
                        "rename_branch" => Ok(UserEvent::RenameBranch),
                        "delete_branch" => Ok(UserEvent::DeleteBranch),
                        "push_branch" => Ok(UserEvent::PushBranch),
                        "pull_branch" => Ok(UserEvent::PullBranch),
                        "create_archive" => Ok(UserEvent::CreateArchive),
                        "unselect_branch" => Ok(UserEvent::UnselectBranch),
                        "copy_branch_name" => Ok(UserEvent::CopyBranchName),
                        "delete_tag" => Ok(UserEvent::DeleteTag),
                        "push_tag" => Ok(UserEvent::PushTag),
                        "copy_tag_name" => Ok(UserEvent::CopyTagName),
                        "apply_stash" => Ok(UserEvent::ApplyStash),
                        "pop_stash" => Ok(UserEvent::PopStash),
                        "drop_stash" => Ok(UserEvent::DropStash),
                        "create_branch_from_stash" => Ok(UserEvent::CreateBranchFromStash),
                        "copy_stash_name" => Ok(UserEvent::CopyStashName),
                        "copy_stash_hash" => Ok(UserEvent::CopyStashHash),
                        _ => {
                            let msg = format!("Unknown user event: {value}");
                            Err(de::Error::custom(msg))
                        }
                    }
                }
            }
        }

        deserializer.deserialize_str(UserEventVisitor)
    }
}

fn parse_user_command_number(s: &str) -> Option<usize> {
    if let Some(num_str) = s.strip_prefix("user_command_") {
        if num_str.parse::<usize>().is_ok() {
            return num_str.parse::<usize>().ok();
        }
        if let Some(num_str) = s.strip_prefix("user_command_view_toggle_") {
            if num_str.parse::<usize>().is_ok() {
                return num_str.parse::<usize>().ok();
            }
        }
    }
    None
}

impl UserEvent {
    pub fn is_countable(&self) -> bool {
        matches!(
            self,
            UserEvent::NavigateUp
                | UserEvent::NavigateDown
                | UserEvent::ScrollUp
                | UserEvent::ScrollDown
                | UserEvent::GoToParent
                | UserEvent::PageUp
                | UserEvent::PageDown
                | UserEvent::HalfPageUp
                | UserEvent::HalfPageDown
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserEventWithCount {
    pub event: UserEvent,
    pub count: usize,
}

impl UserEventWithCount {
    pub fn new(event: UserEvent, count: usize) -> Self {
        Self {
            event,
            count: if count == 0 { 1 } else { count },
        }
    }

    pub fn from_event(event: UserEvent) -> Self {
        Self::new(event, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_event_with_count_new() {
        let event = UserEventWithCount::new(UserEvent::NavigateUp, 5);
        assert_eq!(event.event, UserEvent::NavigateUp);
        assert_eq!(event.count, 5);
    }

    #[test]
    fn test_user_event_with_count_new_zero_count() {
        let event = UserEventWithCount::new(UserEvent::NavigateDown, 0);
        assert_eq!(event.event, UserEvent::NavigateDown);
        assert_eq!(event.count, 1); // zero should be converted to 1
    }

    #[test]
    fn test_user_event_with_count_from_event() {
        let event = UserEventWithCount::from_event(UserEvent::NavigateLeft);
        assert_eq!(event.event, UserEvent::NavigateLeft);
        assert_eq!(event.count, 1);
    }

    #[test]
    fn test_user_event_with_count_equality() {
        let event1 = UserEventWithCount::new(UserEvent::ScrollUp, 3);
        let event2 = UserEventWithCount::new(UserEvent::ScrollUp, 3);
        let event3 = UserEventWithCount::new(UserEvent::ScrollDown, 3);

        assert_eq!(event1, event2);
        assert_ne!(event1, event3);
    }
}
