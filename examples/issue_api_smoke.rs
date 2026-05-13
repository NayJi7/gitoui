//! Live smoke test of the Issues API surface — hits real GitHub.
//! Run with: `cargo run --example issue_api_smoke`
//!
//! Reads the user's stored token from ~/.config/gitoui/github_token.toml,
//! targets the NayJi7/test repo (which the user owns), and exercises
//! every public function in `github::issue`. Prints PASS / FAIL per
//! call. Designed to be re-runnable: each write creates+deletes its
//! own data so the repo state stays clean.

use gitoui::github::issue::{
    add_issue_reaction, close_issue, create_issue, delete_issue_comment, edit_issue_comment,
    fetch_issue_detail, list_issue_timeline, list_issues, list_linked_prs, list_repo_assignees,
    list_repo_labels, list_repo_milestones, load_first_template, post_issue_comment, reopen_issue,
    set_issue_assignees, set_issue_labels, set_issue_milestone, IssueStateReason,
};
use gitoui::github::pr::ReactionKind;
use gitoui::github::RepoCoords;

fn main() {
    let token = std::fs::read_to_string(
        std::env::var("HOME").unwrap() + "/.config/gitoui/github_token.toml",
    )
    .expect("read token file");
    let token = token
        .lines()
        .find_map(|l| {
            l.strip_prefix("token = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .expect("token line not found")
        .to_string();

    let coords = RepoCoords {
        owner: "NayJi7".into(),
        repo: "test".into(),
    };

    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut check = |label: &str, ok: bool, extra: &str| {
        if ok {
            pass += 1;
            println!("  PASS  {} {}", label, extra);
        } else {
            fail += 1;
            println!("  FAIL  {} {}", label, extra);
        }
    };

    println!("== READ SURFACE ==");

    let items = list_issues(&token, &coords);
    check(
        "list_issues",
        items.as_ref().is_ok_and(|v| !v.is_empty()),
        &format!("({} non-PR entries)", items.as_ref().map(|v| v.len()).unwrap_or(0)),
    );
    let items = items.expect("need issue list");
    let target = items.first().expect("at least one issue").number;
    println!("  → target issue #{}", target);

    let detail = fetch_issue_detail(&token, &coords, target);
    check(
        "fetch_issue_detail",
        detail.as_ref().is_ok(),
        &format!(
            "(title={:?}, conv_len={})",
            detail.as_ref().ok().map(|d| d.title.clone()).unwrap_or_default(),
            detail.as_ref().ok().map(|d| d.conversation.len()).unwrap_or(0),
        ),
    );

    let labels = list_repo_labels(&token, &coords);
    check(
        "list_repo_labels",
        labels.as_ref().is_ok_and(|v| !v.is_empty()),
        &format!("({} labels)", labels.as_ref().map(|v| v.len()).unwrap_or(0)),
    );

    let users = list_repo_assignees(&token, &coords);
    check(
        "list_repo_assignees",
        users.as_ref().is_ok_and(|v| !v.is_empty()),
        &format!("({} assignable)", users.as_ref().map(|v| v.len()).unwrap_or(0)),
    );

    let milestones = list_repo_milestones(&token, &coords);
    check(
        "list_repo_milestones",
        milestones.as_ref().is_ok(),
        &format!("({} open)", milestones.as_ref().map(|v| v.len()).unwrap_or(0)),
    );

    let timeline = list_issue_timeline(&token, &coords, target);
    check(
        "list_issue_timeline",
        timeline.as_ref().is_ok(),
        &format!("({} events)", timeline.as_ref().map(|v| v.len()).unwrap_or(0)),
    );

    let linked = list_linked_prs(&token, &coords, target);
    check(
        "list_linked_prs",
        linked.as_ref().is_ok(),
        &format!("({} PRs)", linked.as_ref().map(|v| v.len()).unwrap_or(0)),
    );

    println!("\n== WRITE SURFACE ==");

    // Create a throwaway issue we'll mutate then close.
    let new_num = create_issue(
        &token,
        &coords,
        "smoke: throwaway issue",
        "Body created by issue_api_smoke. Will be closed shortly.",
        &["bug".to_string()],
        &[],
        None,
    );
    check(
        "create_issue",
        new_num.as_ref().is_ok(),
        &format!("(#{:?})", new_num.as_ref().ok()),
    );
    let Ok(new_num) = new_num else {
        eprintln!("aborting writes — create_issue failed");
        std::process::exit(1);
    };

    let post = post_issue_comment(&token, &coords, new_num, "Hi from smoke");
    check("post_issue_comment", post.is_ok(), "");

    // Re-fetch to grab the comment id we just posted.
    let det = fetch_issue_detail(&token, &coords, new_num).expect("re-fetch");
    let comment_id = det
        .conversation
        .iter()
        .find_map(|e| e.id)
        .expect("posted comment must have id");

    let edit = edit_issue_comment(&token, &coords, comment_id, "Hi from smoke (edited)");
    check("edit_issue_comment", edit.is_ok(), "");

    let react = add_issue_reaction(&token, &coords, new_num, ReactionKind::Rocket);
    check("add_issue_reaction (rocket)", react.is_ok(), "");

    let lbl = set_issue_labels(
        &token,
        &coords,
        new_num,
        &["enhancement".to_string()],
    );
    check("set_issue_labels", lbl.is_ok(), "");

    let asn = set_issue_assignees(
        &token,
        &coords,
        new_num,
        &["NayJi7".to_string()],
    );
    check("set_issue_assignees", asn.is_ok(), "");

    let ms = set_issue_milestone(&token, &coords, new_num, None);
    check("set_issue_milestone (None)", ms.is_ok(), "");

    let del = delete_issue_comment(&token, &coords, comment_id);
    check("delete_issue_comment", del.is_ok(), "");

    let close = close_issue(&token, &coords, new_num, IssueStateReason::NotPlanned);
    check(
        "close_issue (not_planned)",
        close.is_ok(),
        &format!("err={:?}", close.err()),
    );

    let reopen = reopen_issue(&token, &coords, new_num);
    check("reopen_issue", reopen.is_ok(), "");

    let close2 = close_issue(&token, &coords, new_num, IssueStateReason::Completed);
    check(
        "close_issue (completed) — cleanup",
        close2.is_ok(),
        &format!("err={:?}", close2.err()),
    );

    println!("\n== LOCAL HELPERS ==");
    let _tpl = load_first_template(&coords);
    check("load_first_template", true, "(returns Option — no crash)");

    println!("\n=== RESULT: {} pass / {} fail ===", pass, fail);
    if fail > 0 {
        std::process::exit(1);
    }
}
