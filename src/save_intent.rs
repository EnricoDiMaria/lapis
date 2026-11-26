//! Pure decisions of the background PDF save lifecycle (no UI, threads or I/O), so the rules
//! that protect against lost edits can be unit-tested.
use std::path::PathBuf;

/// What to do once the PDF has been committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterPdfSave {
    Stay,
    Home,
    Open(PathBuf),
    Exit,
}

impl AfterPdfSave {
    fn is_stay(&self) -> bool { matches!(self, Self::Stay) }
    fn is_exit(&self) -> bool { matches!(self, Self::Exit) }
}

/// A request while a commit is running: Exit is final, a later transition replaces an earlier
/// one, and a plain save is queued at most once (only behind a plain save).
pub fn request_during_job(job_after: &mut AfterPdfSave, queued: &mut Option<AfterPdfSave>, incoming: AfterPdfSave) {
    if incoming.is_stay() {
        if job_after.is_stay() { *queued = Some(AfterPdfSave::Stay); }
    } else if !job_after.is_exit() {
        *job_after = incoming;
        *queued = None;
    }
}

/// A request while nothing runs: the queue keeps the strongest pending destination.
pub fn request_when_idle(queued: &mut Option<AfterPdfSave>, incoming: AfterPdfSave) {
    if queued.as_ref().is_some_and(AfterPdfSave::is_exit) { return; }
    if !incoming.is_stay() || queued.is_none() {
        *queued = Some(incoming);
    }
}

/// A request while a failure prompt is open updates where "Retry" will lead.
pub fn request_during_failure(failure_after: &mut AfterPdfSave, incoming: AfterPdfSave) {
    if !incoming.is_stay() && !failure_after.is_exit() {
        *failure_after = incoming;
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Completion {
    /// Edits arrived after the snapshot and must be committed before leaving.
    Followup,
    /// `newer_edits`: a plain save finished while later edits exist only locally.
    Done { newer_edits: bool },
}

/// Decides what follows a successful commit. `saved` is the content hash of the committed
/// snapshot, `current` that of the live document.
pub fn on_commit_success(
    after: &AfterPdfSave,
    queued: bool,
    saved: Option<u64>,
    current: Option<u64>,
) -> Completion {
    let stale = saved.is_none() || saved != current;
    if (!after.is_stay() || queued) && stale {
        Completion::Followup
    } else {
        Completion::Done { newer_edits: after.is_stay() && saved != current }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AfterPdfSave::*;

    fn open(name: &str) -> AfterPdfSave { Open(name.into()) }

    #[test]
    fn repeated_plain_saves_are_consolidated_into_one_queued_save() {
        let mut job = Stay;
        let mut queued = None;
        for _ in 0..50 { request_during_job(&mut job, &mut queued, Stay); }
        assert_eq!((job, queued), (Stay, Some(Stay)));
        let mut queued = None;
        for _ in 0..50 { request_when_idle(&mut queued, Stay); }
        assert_eq!(queued, Some(Stay));
    }

    #[test]
    fn exit_is_never_downgraded() {
        let mut job = Stay;
        let mut queued = Some(Stay);
        request_during_job(&mut job, &mut queued, Exit);
        assert_eq!((&job, &queued), (&Exit, &None));
        for incoming in [Stay, Home, open("b.pdf"), Exit] {
            request_during_job(&mut job, &mut queued, incoming);
            assert_eq!((&job, &queued), (&Exit, &None));
        }
        let mut queued = Some(Exit);
        for incoming in [Stay, Home, open("b.pdf")] {
            request_when_idle(&mut queued, incoming);
            assert_eq!(queued, Some(Exit));
        }
        let mut failure = Exit;
        for incoming in [Stay, Home, open("b.pdf")] {
            request_during_failure(&mut failure, incoming);
            assert_eq!(failure, Exit);
        }
    }

    #[test]
    fn the_latest_document_transition_wins() {
        let mut job = Home;
        let mut queued = None;
        request_during_job(&mut job, &mut queued, open("b.pdf"));
        assert_eq!(job, open("b.pdf"));
        request_during_job(&mut job, &mut queued, open("c.pdf"));
        assert_eq!(job, open("c.pdf"));
        request_during_job(&mut job, &mut queued, Stay);
        assert_eq!((&job, &queued), (&open("c.pdf"), &None), "a plain save must not queue behind a transition");

        let mut queued = Some(Stay);
        request_when_idle(&mut queued, Home);
        assert_eq!(queued, Some(Home));
        request_when_idle(&mut queued, open("b.pdf"));
        assert_eq!(queued, Some(open("b.pdf")));
        request_when_idle(&mut queued, Stay);
        assert_eq!(queued, Some(open("b.pdf")));

        let mut failure = Home;
        request_during_failure(&mut failure, open("b.pdf"));
        assert_eq!(failure, open("b.pdf"));
    }

    #[test]
    fn a_transition_commits_edits_made_during_the_commit() {
        for after in [Exit, Home, open("b.pdf")] {
            assert_eq!(on_commit_success(&after, false, Some(1), Some(1)), Completion::Done { newer_edits: false });
            assert_eq!(on_commit_success(&after, false, Some(1), Some(2)), Completion::Followup);
            // An unknown snapshot hash can never prove the commit covers the document.
            assert_eq!(on_commit_success(&after, false, None, Some(1)), Completion::Followup);
            assert_eq!(on_commit_success(&after, false, None, None), Completion::Followup);
        }
    }

    #[test]
    fn a_plain_save_reports_newer_local_edits_without_looping() {
        assert_eq!(on_commit_success(&Stay, false, Some(1), Some(2)), Completion::Done { newer_edits: true });
        assert_eq!(on_commit_success(&Stay, false, Some(1), Some(1)), Completion::Done { newer_edits: false });
        // A queued plain save runs again only if something changed.
        assert_eq!(on_commit_success(&Stay, true, Some(1), Some(2)), Completion::Followup);
        assert_eq!(on_commit_success(&Stay, true, Some(1), Some(1)), Completion::Done { newer_edits: false });
    }

    /// Edit during a plain save, then Exit: the exit must wait for a second commit that
    /// covers the edit, and must not complete before it.
    #[test]
    fn close_during_a_save_with_newer_edits_commits_twice_before_exiting() {
        let (mut job, mut queued) = (Stay, None);
        let snapshot = Some(1);
        let live = Some(2); // edit while the worker runs
        request_during_job(&mut job, &mut queued, Exit);
        let was_queued = queued.take().is_some();
        assert_eq!(on_commit_success(&job, was_queued, snapshot, live), Completion::Followup);
        // Followup re-requests with the same destination while idle.
        let mut queue = None;
        request_when_idle(&mut queue, job.clone());
        assert_eq!(queue, Some(Exit));
        let second_snapshot = live; // the second commit snapshots the live state
        assert_eq!(on_commit_success(&Exit, false, second_snapshot, live), Completion::Done { newer_edits: false });
        // ui() returns before any edit once Done, but a later edit would still be caught.
        assert_eq!(on_commit_success(&Exit, false, second_snapshot, Some(3)), Completion::Followup);
    }

    /// Switching documents during a commit with unsaved edits behaves the same way.
    #[test]
    fn switching_documents_during_a_save_waits_for_a_covering_commit() {
        let (mut job, mut queued) = (Stay, None);
        request_during_job(&mut job, &mut queued, open("next.pdf"));
        assert_eq!(job, open("next.pdf"));
        assert_eq!(on_commit_success(&job, false, Some(5), Some(6)), Completion::Followup);
        assert_eq!(on_commit_success(&job, false, Some(6), Some(6)), Completion::Done { newer_edits: false });
    }
}
