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

use crate::{github_auth::GithubAuthState, view::RefreshViewContext};

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
    GithubAuthFinished(GithubAuthState),
    OpenFileDiff {
        hash: String,
        file_path: String,
    },
    CloseDiff,
    CloseDiffToDetail,
    SelectNewerCommit,
    SelectOlderCommit,
    SelectParentCommit,
    CopyToClipboard {
        name: String,
        value: String,
    },
    CopyRawToClipboard {
        value: String,
        success_message: String,
    },
    OpenUrl(String),
    Refresh(RefreshViewContext),
    /// Same as `Refresh` but also bumps the loaded commit count by
    /// `core.option.load_more_count` in the outer `run()` loop.
    LoadMoreCommits(RefreshViewContext),
    /// Open the cumulative diff between two commits (`git diff from..to`).
    /// Triggered by the 2-commit comparison flow once both endpoints are
    /// chosen via Space / Ctrl+click.
    OpenCompareDiff {
        from_hash: String,
        to_hash: String,
    },
    AvatarsUpdated,
    ClearStatusLine,
    UpdateStatusInput(String, Option<u16>, Option<String>),
    NotifyInfo(String),
    NotifySuccess(String),
    NotifyWarn(String),
    NotifyError(String),
    PushCurrentBranch,
    PullCurrentBranch,
    CheckAbortOperation,
    // Phase 2 - Git Actions
    OpenDialog(DialogKind),
    CloseDialog,
    DialogConfirm,
    DialogCancel,
    DialogInput(String),
    ExecuteGitAction {
        target: String,
        action: GitAction,
    },
    OpenBranchDetail {
        branch_name: String,
    },
    OpenSetUpstreamDialog {
        branch: String,
    },
    OpenTagDetail {
        tag_name: String,
    },
    StageFile {
        file: String,
    },
    UnstageFile {
        file: String,
    },
    DiscardFile {
        file: String,
    },
    RefreshUncommitted,
    OpenUncommittedDiff {
        file_path: String,
        is_staged: bool,
    },
    /// Toggle a single hunk between staged and unstaged. `currently_staged`
    /// drives which direction the patch is applied — staged hunks reverse the
    /// patch (unstage), unstaged hunks forward-apply it (stage).
    ToggleHunkStage {
        file_path: String,
        hunk_idx: usize,
        currently_staged: bool,
    },
    CloseDiffToUncommitted,
    Tick,
    BackgroundFetch,
    OpenFileHistory {
        file_path: String,
    },
    OpenStashDiff {
        stash_ref: String,
    },
    CloseFileHistory,
    OpenBlame {
        file_path: String,
    },
    CloseBlame,
    OpenConflictEditor {
        file_path: String,
    },
    CloseConflictEditor,
    OpenInteractiveRebase {
        /// Commit the user is rebasing ONTO — `base..HEAD` commits get
        /// loaded into the editor.
        base_hash: String,
    },
    CloseInteractiveRebase,
    /// Open the GitHub Pull Requests view. App resolves auth + remote
    /// and surfaces an error notification if either is missing.
    OpenPullRequests,
    /// Open the PR view and jump straight to the given PR's detail —
    /// dispatched from cross-view navigation (e.g. clicking a `#N`
    /// reference inside an Issue comment).
    OpenPullRequestDetail { number: u64 },
    /// Cross-view nav from a `#N` mention click/Enter — switches
    /// to the Issues view and opens that issue's detail page.
    OpenIssueDetail { number: u64 },
    /// PR view's `#N` resolver / mention popup feed — full issue list
    /// fetched in the background so PR comments can colour `#N`
    /// references AND the `#` autocomplete popup has titles to show.
    PrMentionIssuesFetched {
        issues: Vec<(u64, String)>,
    },
    /// Issue view: reaction add succeeded — carries the new
    /// reaction's id so the view can record `(kind, id)` against
    /// `(issue_number, target_idx)` and later DELETE on toggle-off.
    IssueReactionApplied {
        issue_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
        reaction_id: u64,
    },
    /// Issue view: reaction removal succeeded — view drops the
    /// recorded `(kind, id)` pair.
    IssueReactionRemoved {
        issue_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
    },
    /// PR view: reaction add succeeded — same shape as the Issue
    /// counterpart, just routed to the PR view.
    PrReactionApplied {
        pr_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
        reaction_id: u64,
    },
    PrReactionRemoved {
        pr_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
    },
    /// Viewer's pre-existing reactions on a comment/body — fetched
    /// when the reaction picker opens so the chips can be tagged
    /// with their red "mine" indicator even on a freshly-loaded
    /// session.
    IssueViewerReactionsFetched {
        issue_number: u64,
        target_idx: usize,
        reactions: Vec<(crate::github::pr::ReactionKind, u64)>,
    },
    PrViewerReactionsFetched {
        pr_number: u64,
        target_idx: usize,
        reactions: Vec<(crate::github::pr::ReactionKind, u64)>,
    },
    /// Close the PR view, returning to the previous list state.
    ClosePullRequests,
    /// Open the GitHub Issues view (Shift+I from the commit list).
    /// App resolves auth + remote and notifies on failure.
    OpenIssues,
    /// Close the Issues view, returning to the previous list state.
    CloseIssues,
    /// Background fetch of an issue's full detail completed.
    IssueDetailFetched {
        number: u64,
        result: Result<crate::github::issue::IssueDetail, String>,
    },
    /// Result of a background write on an issue (comment / close /
    /// labels / etc.) — sent by the worker thread that ran the
    /// PATCH/POST/DELETE.
    IssueActionDone {
        number: u64,
        action: String,
        result: Result<(), String>,
    },
    /// Issue's Timeline tab finished loading.
    IssueTimelineFetched {
        number: u64,
        result: Result<Vec<crate::github::issue::TimelineEvent>, String>,
    },
    /// Issue's Linked-PRs tab finished loading.
    IssueLinkedFetched {
        number: u64,
        result: Result<Vec<crate::github::issue::LinkedPr>, String>,
    },
    /// PR list cached for the `#` autocomplete in the Issues view's
    /// comment editor. Fetched once lazily when the user types `#`
    /// for the first time per session.
    IssueMentionPrsFetched {
        result: Result<Vec<crate::github::pr::PullRequest>, String>,
    },
    /// User asked to open the issue labels picker. The handler fetches
    /// repo labels in the background and opens the dialog.
    OpenIssueLabelsPicker {
        issue_number: u64,
        issue_title: String,
        all_labels: Vec<crate::github::pr::Label>,
        currently_on_issue: Vec<String>,
    },
    /// Same as labels but for assignees.
    OpenIssueAssigneesPicker {
        issue_number: u64,
        issue_title: String,
        all_users: Vec<String>,
        currently_assigned: Vec<String>,
    },
    /// Milestone picker — single-select.
    OpenIssueMilestonePicker {
        issue_number: u64,
        issue_title: String,
        all_milestones: Vec<crate::github::issue::Milestone>,
        currently_set: Option<u64>,
    },
    /// Issue created — view jumps straight into the new issue's detail.
    IssueCreated {
        number: u64,
    },
    /// Compose-issue picker dialogs return their selections via these
    /// events — the view captures them into ComposeState rather than
    /// firing a SetIssueX write (no issue exists yet).
    ComposeIssueLabelsPicked { labels: Vec<String> },
    ComposeIssueAssigneesPicked { assignees: Vec<String> },
    ComposeIssueMilestonePicked { milestone: Option<u64> },
    /// Issue write actions dispatched from the issue view / dialogs.
    SetIssueLabels { issue_number: u64, labels: Vec<String> },
    SetIssueAssignees { issue_number: u64, assignees: Vec<String> },
    SetIssueMilestone { issue_number: u64, milestone: Option<u64> },
    CloseIssueWithReason {
        issue_number: u64,
        reason: crate::github::issue::IssueStateReason,
    },
    ReopenIssue { issue_number: u64 },
    DeleteIssueComment { issue_number: u64, comment_id: u64 },
    /// Background fetch of a PR's full detail completed — pushed by the
    /// worker thread the PR view spawned. The view updates its cache and
    /// re-renders.
    PullRequestDetailFetched {
        number: u64,
        result: Result<crate::github::pr::PullRequestDetail, String>,
    },
    /// Result of a background write action on a PR — sent by the worker
    /// thread that ran the POST/PATCH/DELETE. The view shows a toast,
    /// invalidates its cache for `number`, and triggers a re-fetch.
    PullRequestActionDone {
        number: u64,
        action: String,
        result: Result<(), String>,
    },
    /// Repo labels finished fetching for the picker — payload carries
    /// the full label set (with their hex colours so the dialog can
    /// paint GitHub-style chips), plus which names are currently
    /// attached to the PR. The handler opens the multi-select dialog.
    OpenPrLabelsPicker {
        pr_number: u64,
        pr_title: String,
        all_labels: Vec<crate::github::pr::Label>,
        currently_on_pr: Vec<String>,
    },
    /// Same shape as `OpenPrLabelsPicker` but for reviewers — uses the
    /// `/assignees` endpoint as the user pool.
    OpenPrReviewersPicker {
        pr_number: u64,
        pr_title: String,
        all_users: Vec<String>,
        currently_requested: Vec<String>,
    },
    /// A background fetch of a single commit's full detail completed.
    /// The PR view stores it under its SHA and renders the drill-down
    /// page if the user is still pointed at it.
    PrCommitDetailFetched {
        sha: String,
        result: Result<crate::github::pr::CommitDetail, String>,
    },
    /// Open the existing CommitDetail view (`View::Detail`) for a PR
    /// commit. The app `git fetches` the PR ref first if the commit
    /// isn't already locally available, then transitions. The PR
    /// number is remembered so `Esc` returns straight to the PR view
    /// instead of dropping to the commit graph.
    OpenPrCommitDetail { pr_number: u64, sha: String },
    /// Open the existing DiffView for a single file in a PR. Same
    /// fetch / return-flow mechanics as `OpenPrCommitDetail`.
    OpenPrFileDiff {
        pr_number: u64,
        sha: String,
        file_path: String,
    },
    /// A newly-created PR landed on GitHub — the PR view clears its
    /// compose draft, reloads the list, and opens the freshly
    /// created PR in Detail mode.
    PrCreated { number: u64 },
    /// Compose form's labels-picker fetch finished — app opens the
    /// multi-select dialog with this payload, and on confirm
    /// dispatches `ComposeLabelsPicked` back to the view.
    OpenComposeLabelsPicker {
        all_labels: Vec<crate::github::pr::Label>,
        currently_selected: Vec<String>,
    },
    /// Labels the user chose in the compose-form picker — pushed
    /// back into the active `ComposeState`.
    ComposeLabelsPicked { labels: Vec<crate::github::pr::Label> },
    OpenDetailByHash {
        hash: String,
    },
    SwitchWorktree {
        path: String,
    },
    /// Sent (debounced) when the .git directory changes — triggers a refresh
    /// of the current view if the user isn't in an input/dialog state.
    FilesystemChanged,
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
    /// Confirm squashing a commit into its parent. No options — just a
    /// confirmation prompt before we run the rebase under the hood.
    Squash { target: String },
    /// Merge a pull request — radio for merge method + optional title
    /// and message. Confirm runs `PUT /pulls/{n}/merge`.
    MergePullRequest {
        number: u64,
        pr_title: String,
        pr_body: String,
    },
    /// Confirm before deleting a PR comment — preview shows the body
    /// and author so the user sees which comment is about to go.
    ConfirmDeleteComment {
        pr_number: u64,
        comment_id: u64,
        is_review: bool,
        author: String,
        body_preview: String,
    },
    /// Confirm closing or reopening a PR. `closing == true` closes a
    /// currently-open PR; `false` reopens a closed one.
    ConfirmPullRequestStateChange {
        pr_number: u64,
        pr_title: String,
        closing: bool,
    },
    /// Confirm flipping draft ↔ ready. `to_draft == true` converts
    /// a ready PR back to draft; `false` marks a draft as ready.
    ConfirmPullRequestDraftToggle {
        pr_number: u64,
        pr_title: String,
        node_id: String,
        to_draft: bool,
    },
    /// Multi-select picker for the PR's labels. `selected` is the
    /// set currently checked (mutated as the user toggles); on confirm
    /// it becomes the new label set on the PR. Labels carry their
    /// hex colours so the dialog can paint GitHub-style chips.
    PullRequestLabels {
        pr_number: u64,
        pr_title: String,
        all_labels: Vec<crate::github::pr::Label>,
        selected: Vec<bool>,
        /// When `true`, the dialog is being used by the compose-PR
        /// form: confirm sends `ComposeLabelsPicked` back to the
        /// view instead of dispatching `SetPullRequestLabels`.
        for_compose: bool,
    },
    /// Multi-select picker for the PR's reviewers. Same shape as
    /// labels — `selected` starts at currently-requested reviewers.
    PullRequestReviewers {
        pr_number: u64,
        pr_title: String,
        all_users: Vec<String>,
        selected: Vec<bool>,
        initial: Vec<bool>,
    },
    /// Issue labels multi-select. Mirror of `PullRequestLabels` —
    /// kept distinct so the dialog confirm dispatches to the issue
    /// view rather than the PR view.
    IssueLabels {
        issue_number: u64,
        issue_title: String,
        all_labels: Vec<crate::github::pr::Label>,
        selected: Vec<bool>,
        /// When `true`, this is the Compose-new-issue picker — the
        /// dialog stays in-memory rather than firing the SetIssueLabels
        /// write (the compose state collects the picked labels for the
        /// eventual POST /issues).
        for_compose: bool,
    },
    /// Issue assignees multi-select. Same shape as reviewers.
    IssueAssignees {
        issue_number: u64,
        issue_title: String,
        all_users: Vec<String>,
        selected: Vec<bool>,
        initial: Vec<bool>,
        for_compose: bool,
    },
    /// Issue milestone single-select. `selected` tracks the picked
    /// milestone number, or `None` for "no milestone".
    IssueMilestone {
        issue_number: u64,
        issue_title: String,
        all_milestones: Vec<crate::github::issue::Milestone>,
        selected: Option<u64>,
        for_compose: bool,
    },
    /// Confirmation dialog when closing an issue — lets the user pick
    /// "Completed" vs "Not planned" before firing the write.
    ConfirmCloseIssue {
        issue_number: u64,
        issue_title: String,
    },
    /// Confirmation dialog when reopening a closed issue — single
    /// yes/no, no state_reason to pick (GitHub clears it on reopen).
    ConfirmReopenIssue {
        issue_number: u64,
        issue_title: String,
    },
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
    // Remote actions
    AddRemote,
    ConfirmDeleteRemote { name: String },
    ChooseRemote { remotes: Vec<String>, branch: String },
    SetUpstream { remotes: Vec<String>, branch: String },
    // Abort in-progress operation confirmation
    ConfirmAbortOperation { op_name: String },
    // Amend HEAD commit message
    AmendMessage { current_message: String },
    // Worktree switch confirmation
    ConfirmSwitchWorktree { path: String, display_name: String },
    // Worktree delete confirmation
    ConfirmDeleteWorktree { path: String, display_name: String, is_dirty: bool },
    // Add new worktree
    AddWorktree,
    // Checkout blocked by local changes — pick a resolution path.
    CheckoutHasLocalChanges { target: String, is_branch: bool },
}

#[derive(Debug, Clone)]
pub enum GitAction {
    // Commit actions
    Checkout,
    /// Discard all local changes, then checkout the target.
    CheckoutDiscard,
    /// Stash local changes, then checkout the target.
    CheckoutStash,
    CreateBranch {
        name: String,
        checkout: bool,
    },
    AddTag {
        name: String,
        annotated: bool,
        message: Option<String>,
    },
    CherryPick {
        no_commit: bool,
        record_origin: bool,
    },
    Revert,
    Drop,
    Merge {
        no_ff: bool,
        squash: bool,
        no_commit: bool,
    },
    Rebase {
        ignore_date: bool,
        interactive: bool,
    },
    /// Squash with parent — non-interactive, no options.
    SquashWithParent,
    /// Merge a PR via the GitHub API. `method` is "merge" / "squash" /
    /// "rebase" (matches GitHub's `merge_method` body field).
    MergePullRequest { method: String },
    /// Delete a PR comment after user confirmation. `target` carries
    /// the comment id; `is_review` picks between the issue-comments
    /// and pulls-comments endpoint.
    DeletePrComment { pr_number: u64, is_review: bool },
    /// Close (without merge) or reopen a PR via REST PATCH.
    SetPullRequestState { pr_number: u64, state: String },
    /// Flip the draft flag on a PR. Uses GraphQL.
    SetPullRequestDraft { pr_number: u64, node_id: String, draft: bool },
    /// Replace the PR's full label set.
    SetPullRequestLabels { pr_number: u64, labels: Vec<String> },
    /// Add and/or remove reviewers on the PR.
    SetPullRequestReviewers {
        pr_number: u64,
        to_add: Vec<String>,
        to_remove: Vec<String>,
    },
    Reset {
        mode: String,
    },
    // Branch actions
    DeleteBranch {
        force: bool,
    },
    RenameBranch {
        new_name: String,
    },
    PushBranch {
        force: bool,
    },
    PullBranch {
        rebase: bool,
    },
    Fetch,
    CreateArchive,
    // Tag actions
    DeleteTag,
    PushTag,
    // Stash actions
    ApplyStash,
    PopStash,
    DropStash,
    CreateBranchFromStash {
        branch_name: String,
    },
    // Uncommitted actions
    StageFile {
        file: String,
    },
    StageAll,
    UnstageFile {
        file: String,
    },
    UnstageAll,
    DiscardFile {
        file: String,
    },
    DiscardAll,
    Stash {
        message: Option<String>,
        include_untracked: bool,
    },
    Commit {
        message: String,
        amend: bool,
    },
    CleanUntracked,
    Push,
    // Abort in-progress operations
    AbortRebase,
    AbortMerge,
    AbortCherryPick,
    // Remote actions
    AddRemote { url: String },  // remote name comes from `target` in execute_git_action
    RemoveRemote,               // remote name comes from `target`
    PushSetUpstream { branch: String }, // target = remote name
    SetUpstream { branch: String }, // target = remote name
    AddWorktree {
        name: String,
        checkout: bool,
    },
    DeleteWorktree {
        force: bool,
    },
}

#[derive(Clone)]
pub struct Sender {
    tx: mpsc::Sender<AppEvent>,
}

impl Sender {
    // `mpsc::channel` is unbounded, so the only failure mode is a closed
    // channel — which only happens after the receiver has been dropped, i.e.
    // during final shutdown when nothing will consume the event anyway.
    // Silently dropping is correct in that race; panicking on the spawned
    // poller thread (or any view-side sender) just pollutes the terminal
    // after teardown.
    pub fn send(&self, event: AppEvent) {
        let _ = self.tx.send(event);
    }

    pub fn try_send(&self, event: AppEvent) {
        let _ = self.tx.send(event);
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

    fn recv_timeout(&self, timeout: std::time::Duration) -> Result<AppEvent, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
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
        let handle = thread::spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                match ratatui::crossterm::event::poll(std::time::Duration::from_millis(50)) {
                    Ok(true) => {
                        match ratatui::crossterm::event::read() {
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
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        panic!("Failed to poll event: {e}");
                    }
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

    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<AppEvent, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
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
    Push,
    Pull,
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
    /// Squash the selected commit with its parent — runs an interactive
    /// rebase under the hood with a `[pick parent, fixup target]` plan.
    /// Faster than opening the rebase editor for the common "combine the
    /// last few wip/fix commits" case.
    Squash,
    /// Open the GitHub Pull Requests view — gated behind an authenticated
    /// GitHub session.
    PullRequests,
    /// Open the GitHub Issues view — gated behind an authenticated
    /// GitHub session. Default keybind: Shift+I.
    Issues,
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
    // Diff view file cycling
    CycleFileNext,
    CycleFilePrev,
    // Branch upstream
    SetUpstream,
    // Abort in-progress rebase/merge/cherry-pick
    AbortOperation,
    // Amend HEAD commit message from detail view
    AmendCommit,
    // Open file history (git log --follow)
    FileHistory,
    // Open git blame for the file in the current context.
    Blame,
    // Open the inline change-directory overlay (header turns into a text
    // input with a recents + filesystem autocomplete dropdown).
    ChangeDir,
    // Load more commits (extends the initial_load_count by load_more_count)
    LoadMore,
    // 2-commit comparison: Space / Ctrl+click toggles a "marked" commit
    // and, on a second commit, opens the cumulative diff between them.
    MarkCompare,
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
                        "load_more" => Ok(UserEvent::LoadMore),
                        "mark_compare" => Ok(UserEvent::MarkCompare),
                        "push" => Ok(UserEvent::Push),
                        "pull" => Ok(UserEvent::Pull),
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
                        "squash" => Ok(UserEvent::Squash),
                        "pull_requests" => Ok(UserEvent::PullRequests),
                        "issues" => Ok(UserEvent::Issues),
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
                        "cycle_file_next" => Ok(UserEvent::CycleFileNext),
                        "cycle_file_prev" => Ok(UserEvent::CycleFilePrev),
                        "set_upstream" => Ok(UserEvent::SetUpstream),
                        "abort_operation" => Ok(UserEvent::AbortOperation),
                        "amend_commit" => Ok(UserEvent::AmendCommit),
                        "file_history" => Ok(UserEvent::FileHistory),
                        "blame" => Ok(UserEvent::Blame),
                        "change_dir" => Ok(UserEvent::ChangeDir),
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

    // Regression: dropping the receiver before the spawned event-poller thread
    // (or any view-side `Sender::send` call) used to panic with `SendError`.
    // See src/event.rs Sender::send doc — silent drop is correct on shutdown.
    #[test]
    fn sender_send_after_receiver_drop_does_not_panic() {
        let (tx, rx) = mpsc::channel();
        let sender = Sender { tx };
        drop(rx);
        sender.send(AppEvent::Tick);
        sender.send(AppEvent::Quit);
    }
}
