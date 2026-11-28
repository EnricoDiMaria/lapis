use super::*;

#[cfg(test)]
mod fit_notes_layout_tests {
    use super::{Lapis, ZoomMode};

    #[test]
    fn divider_double_click_needs_no_adjustment_when_fitted() {
        assert!(!Lapis::notes_layout_needs_fit(
            ZoomMode::FitPage,
            350.0,
            350.0
        ));
        assert!(!Lapis::notes_layout_needs_fit(
            ZoomMode::FitPage,
            350.0,
            352.0
        ));
        assert!(Lapis::notes_layout_needs_fit(
            ZoomMode::FitPage,
            350.0,
            354.0
        ));
        assert!(Lapis::notes_layout_needs_fit(
            ZoomMode::FitPage,
            354.0,
            350.0
        ));
        assert!(Lapis::notes_layout_needs_fit(
            ZoomMode::Custom,
            350.0,
            350.0
        ));
        assert!(Lapis::notes_layout_needs_fit(
            ZoomMode::FitWidth,
            350.0,
            350.0
        ));
    }

    #[test]
    fn gives_unused_width_to_notes() {
        assert_eq!(
            Lapis::fitted_notes_width(1600.0, 802.0, 1.0, 1, 250.0, 1500.0),
            798.0
        );
    }

    #[test]
    fn preserves_minimum_notes_width() {
        assert_eq!(
            Lapis::fitted_notes_width(1200.0, 802.0, 2.0, 1, 250.0, 1100.0),
            250.0
        );
    }

    #[test]
    fn accounts_for_book_spine() {
        assert_eq!(
            Lapis::fitted_notes_width(1800.0, 802.0, 0.75, 2, 250.0, 1700.0),
            596.0
        );
    }
}

#[cfg(test)]
mod single_page_wheel_tests {
    use super::{SinglePageWheel, wheel_page_target};
    use std::collections::HashSet;

    #[test]
    fn turns_only_at_the_requested_edge() {
        let mut wheel = SinglePageWheel::default();
        assert_eq!(wheel.update(1.0, -96.0, 20.0, 100.0), 0);
        assert_eq!(wheel.update(1.1, 96.0, 100.0, 100.0), 0);
        assert_eq!(wheel.update(1.2, -96.0, 100.0, 100.0), 2);
        assert_eq!(wheel.update(1.21, 48.0, 0.0, 100.0), -1);
    }

    #[test]
    fn preserves_remainder_without_a_cooldown() {
        let mut wheel = SinglePageWheel::default();
        assert_eq!(wheel.update(1.0, -120.0, 0.0, 0.0), 2);
        assert_eq!(wheel.update(1.01, -24.0, 0.0, 0.0), 1);
        assert_eq!(wheel.update(1.02, -48.0, 0.0, 0.0), 1);
    }

    #[test]
    fn small_trackpad_events_accumulate_like_one_large_event() {
        let mut wheel = SinglePageWheel::default();
        let turns: i32 = (0..24)
            .map(|i| wheel.update(1.0 + f64::from(i) * 0.01, -8.0, 0.0, 0.0))
            .sum();
        assert_eq!(turns, 4);
        let mut wheel = SinglePageWheel::default();
        assert_eq!(wheel.update(1.0, -192.0, 0.0, 0.0), turns);
    }

    #[test]
    fn idle_and_reversal_discard_partial_distance() {
        let mut wheel = SinglePageWheel::default();
        assert_eq!(wheel.update(1.0, -30.0, 0.0, 0.0), 0);
        assert_eq!(wheel.update(1.4, -30.0, 0.0, 0.0), 0);
        assert_eq!(wheel.update(1.41, 30.0, 0.0, 0.0), 0);
        assert_eq!(wheel.update(1.42, 18.0, 0.0, 0.0), -1);
    }

    #[test]
    fn skips_hidden_pages_and_stops_at_document_boundaries() {
        let hidden = HashSet::from([3, 5, 9]);
        assert_eq!(wheel_page_target(2, 9, &hidden, 3), 7);
        assert_eq!(wheel_page_target(7, 9, &hidden, -3), 2);
        assert_eq!(wheel_page_target(7, 9, &hidden, 100), 8);
        assert_eq!(wheel_page_target(2, 9, &hidden, -100), 1);
        assert_eq!(wheel_page_target(8, 9, &hidden, 1), 8);
    }
}

#[cfg(test)]
mod release_correctness_tests {
    use super::*;
    use crate::docid;
    use crate::test_support::isolated_dir;
    use crate::workspace::audio_session::replace_asr_channel;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn invalid_candidate_keeps_active_document_until_successful_switch() {
        let root = isolated_dir!(invalid_candidate_keeps_active_document_until_successful_switch);
        let first = root.join("first.pdf");
        let second = root.join("second.pdf");
        let invalid = root.join("invalid.pdf");
        notes::create_blank_study_notebook(&first).unwrap();
        notes::create_blank_study_notebook(&second).unwrap();
        std::fs::write(&invalid, b"not a PDF").unwrap();

        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        assert!(app.open_pdf(first.clone(), &ctx));
        app.current_note = "Only in the first document".into();
        app.sync_local_checkpoint().unwrap();
        let (old_tx, old_cancel) = app.new_asr_job();

        assert!(!app.open_pdf(invalid.clone(), &ctx));
        assert_eq!(app.pdf_path, first);
        assert_eq!(app.current_note, "Only in the first document");
        assert!(!old_cancel.load(Ordering::Relaxed));
        assert!(matches!(
            doc_lock::acquire(&first),
            Err(doc_lock::LockError::Busy)
        ));
        assert!(doc_lock::acquire(&invalid).is_ok());

        old_tx.send(audio::AsrJobResult::Cancelled).unwrap();
        assert!(app.open_pdf(second.clone(), &ctx));
        assert_eq!(app.pdf_path, second);
        assert!(app.current_note.is_empty());
        assert!(old_cancel.load(Ordering::Relaxed));
        assert!(old_tx.send(audio::AsrJobResult::Cancelled).is_err());
        assert!(app.asr_rx.try_recv().is_err());
        assert!(doc_lock::acquire(&first).is_ok());
        assert!(matches!(
            doc_lock::acquire(&second),
            Err(doc_lock::LockError::Busy)
        ));
        assert_eq!(
            notes::load_doc_data(&first).notes[&1],
            "Only in the first document"
        );
    }

    #[test]
    fn failed_audio_index_write_keeps_document_open_until_retry() {
        let root = isolated_dir!(failed_audio_index_write_keeps_document_open_until_retry);
        let pdf = root.join("lecture.pdf");
        notes::create_blank_study_notebook(&pdf).unwrap();
        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        assert!(app.open_pdf(pdf.clone(), &ctx));

        let media_dir = AudioDocIndex::media_dir_for_pdf(&pdf);
        let index = media_dir.join("audio_index.json");
        app.audio_index.lecture_title = "Saved title".into();
        app.persist_audio_index().unwrap();
        let before = std::fs::read(&index).unwrap();
        let blocked = media_dir.join(format!("audio_index.json.{}.tmp", std::process::id()));
        std::fs::create_dir(&blocked).unwrap();

        app.audio_index.lecture_title = "Pending title".into();
        assert!(app.persist_audio_index().is_err());
        assert!(!app.complete_pdf_save(AfterPdfSave::Home, &ctx));
        assert!(app.audio_index_pending);
        assert_eq!(app.pdf_path, pdf);
        assert_eq!(std::fs::read(&index).unwrap(), before);
        assert!(matches!(
            doc_lock::acquire(&pdf),
            Err(doc_lock::LockError::Busy)
        ));

        std::fs::remove_dir(&blocked).unwrap();
        assert!(app.complete_pdf_save(AfterPdfSave::Home, &ctx));
        assert!(!app.has_pdf());
        assert!(doc_lock::acquire(&pdf).is_ok());
        assert_eq!(AudioDocIndex::load(&pdf).lecture_title, "Pending title");
    }

    #[test]
    fn late_image_jobs_cannot_cross_document_or_page_changes() {
        let root = isolated_dir!(late_image_jobs_cannot_cross_document_or_page_changes);
        let first = root.join("first.pdf");
        let second = root.join("second.pdf");
        notes::create_blank_study_notebook(&first).unwrap();
        notes::create_blank_study_notebook(&second).unwrap();
        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        assert!(app.open_pdf(first, &ctx));
        let target = ShotTarget::Note { page: 1, cursor: 0 };
        let switched = app.shots.pending_for_test(target.clone());
        assert!(app.open_pdf(second.clone(), &ctx));
        assert!(switched.send(None).is_err());
        let reopened = app.shots.pending_for_test(target.clone());
        assert!(app.open_pdf(second, &ctx));
        assert!(reopened.send(None).is_err());

        app.insert_blank_page_after(1, notes::BlankPageStyle::Blank, &ctx);
        assert_eq!(app.total_pages, 2);
        app.push_undo_snapshot(1);
        app.selected_textbox = Some(0);
        let moved = app.shots.pending_for_test(target.clone());
        app.reorder_pdf_page(1, 2, &ctx);
        assert_eq!(app.current_page, 1);
        assert!(app.undo_stack.is_empty());
        assert!(app.selected_textbox.is_none());
        assert!(moved.send(None).is_err());
        let closed = app.shots.pending_for_test(target);
        assert!(app.complete_pdf_save(AfterPdfSave::Home, &ctx));
        assert!(closed.send(None).is_err());
    }

    #[test]
    fn deleting_a_custom_page_with_only_a_sketch_or_memo_requires_confirmation() {
        let root =
            isolated_dir!(deleting_a_custom_page_with_only_a_sketch_or_memo_requires_confirmation);
        let pdf = root.join("study.pdf");
        notes::create_blank_study_notebook(&pdf).unwrap();
        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        assert!(app.open_pdf(pdf, &ctx));
        app.insert_blank_page_after(1, notes::BlankPageStyle::Blank, &ctx);
        assert!(app.is_disposable_blank_page(2));

        let mut sketch = notes::SketchPage::new(notes::BlankPageStyle::Blank);
        sketch.strokes.push(annotations::PageAnnotation {
            kind: annotations::AnnotationKind::Freehand,
            color: [0, 0, 0],
            points: vec![(10.0, 20.0), (20.0, 30.0)],
            text: String::new(),
            font_size: 18.0,
            bold: false,
            italic: false,
            stroke_width: 2.5,
            native_source: None,
        });
        app.sketches.insert(2, sketch);
        app.request_delete_page(2, &ctx);
        assert_eq!(app.pending_delete_page, Some(2));
        assert_eq!(app.total_pages, 2);

        app.pending_delete_page = None;
        app.sketches.clear();
        app.audio_index.page_memos.insert(
            2,
            vec![audio::VoiceMemoEntry {
                id: "memo".into(),
                filename: "memo.wav".into(),
                duration_ms: 100,
                transcript: None,
                title: None,
            }],
        );
        app.request_delete_page(2, &ctx);
        assert_eq!(app.pending_delete_page, Some(2));
        assert_eq!(app.total_pages, 2);
    }

    #[test]
    fn failed_media_updates_preserve_the_recording_and_published_index() {
        let root = isolated_dir!(failed_media_updates_preserve_the_recording_and_published_index);
        let pdf = root.join("lecture.pdf");
        notes::create_blank_study_notebook(&pdf).unwrap();
        let ctx = egui::Context::default();
        let mut app = Lapis::new(&eframe::CreationContext::_new_kittest(ctx.clone()), None);
        assert!(app.open_pdf(pdf.clone(), &ctx));
        let dir = AudioDocIndex::media_dir_for_pdf(&pdf);
        let lecture = dir.join("original.wav");
        let mut wav = hound::WavWriter::create(
            &lecture,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in 0..64_000 {
            let voice = !(11_200..52_800).contains(&sample);
            wav.write_sample(if voice { 1000i16 } else { 0 }).unwrap();
        }
        wav.finalize().unwrap();
        app.audio_index.lecture_wav = Some("original.wav".into());
        app.audio_index.lecture_duration_ms = 4_000;
        app.audio_index.timeline = vec![(0, 1)];
        app.audio_index.segments = vec![audio::TranscriptSegment {
            start_ms: 0,
            end_ms: 150,
            page: 1,
            text: "Existing transcript".into(),
        }];
        app.audio_index.page_memos.insert(
            1,
            vec![audio::VoiceMemoEntry {
                id: "memo".into(),
                filename: "original.wav".into(),
                duration_ms: 4_000,
                transcript: Some("Existing memo".into()),
                title: None,
            }],
        );
        app.persist_audio_index().unwrap();
        let original = std::fs::read(&lecture).unwrap();
        let index_path = dir.join("audio_index.json");
        let before = std::fs::read(&index_path).unwrap();

        app.finish_import_lecture_media(root.join("no-input.wav"), &dir);
        assert_eq!(std::fs::read(&lecture).unwrap(), original);
        assert_eq!(std::fs::read(&index_path).unwrap(), before);
        let blocked = dir.join(format!("audio_index.json.{}.tmp", std::process::id()));
        std::fs::create_dir(&blocked).unwrap();
        let replacement = dir.join("candidate.wav");
        std::fs::copy(&lecture, &replacement).unwrap();
        assert!(
            app.install_imported_lecture(&dir, &replacement, 4_000)
                .is_err()
        );
        app.trim_start_ms = 0;
        app.trim_end_ms = 100;
        app.apply_audio_trim(&lecture);
        app.remove_lecture_silence(&dir, &lecture);
        assert!(app.delete_memo_recording(&dir, 1, 0).is_err());
        assert_eq!(std::fs::read(&lecture).unwrap(), original);
        assert_eq!(std::fs::read(&index_path).unwrap(), before);
        assert_eq!(app.audio_index.segments[0].text, "Existing transcript");
        assert_eq!(
            app.audio_index.page_memos[&1][0].transcript.as_deref(),
            Some("Existing memo")
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 4);

        std::fs::remove_dir(&blocked).unwrap();
        app.install_imported_lecture(&dir, &replacement, 4_000)
            .unwrap();
        assert!(lecture.exists(), "the memo still references this WAV");
        app.delete_memo_recording(&dir, 1, 0).unwrap();
        assert!(!lecture.exists());
        assert_eq!(
            AudioDocIndex::load(&pdf).lecture_wav.as_deref(),
            Some("candidate.wav")
        );
    }

    #[test]
    fn retired_transcription_cannot_reach_the_next_job() {
        let (old_tx, mut rx) = channel();
        let old_cancel = Arc::new(AtomicBool::new(false));
        let mut cancel = old_cancel.clone();
        assert!(old_tx.send(audio::AsrJobResult::Cancelled).is_ok());
        let new_tx = replace_asr_channel(&mut rx, &mut cancel);
        assert!(old_cancel.load(Ordering::Relaxed));
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(old_tx.send(audio::AsrJobResult::Cancelled).is_err());
        assert!(rx.try_recv().is_err());
        assert!(
            new_tx
                .send(audio::AsrJobResult::Error("new job".into()))
                .is_ok()
        );
        assert!(matches!(rx.try_recv(), Ok(audio::AsrJobResult::Error(s)) if s == "new job"));
        let _third = replace_asr_channel(&mut rx, &mut cancel);
        assert!(new_tx.send(audio::AsrJobResult::Cancelled).is_err());
    }

    #[test]
    fn import_refreshes_the_locked_note_page_not_the_pdf_page() {
        let mut notes = HashMap::from([(1, "old one".to_owned()), (2, "old two".to_owned())]);
        let editor = merge_imported_notes(&mut notes, [(1, "new one".to_owned())], 2);
        assert_eq!(editor, "old two");
        assert_eq!(notes.get(&1).unwrap(), "new one");
        let editor = merge_imported_notes(&mut notes, [(2, "new two".to_owned())], 2);
        assert_eq!(editor, "new two");
    }

    #[test]
    fn sketch_or_memo_is_study_content() {
        let mut sketch = notes::SketchPage::new(notes::BlankPageStyle::Blank);
        assert!(!has_extra_study_content(Some(&sketch), None));
        sketch.strokes.push(annotations::PageAnnotation {
            kind: annotations::AnnotationKind::Freehand,
            color: [0, 0, 0],
            points: vec![(1.0, 1.0), (4.0, 4.0)],
            text: String::new(),
            font_size: 18.0,
            bold: false,
            italic: false,
            stroke_width: 2.5,
            native_source: None,
        });
        assert!(has_extra_study_content(Some(&sketch), None));
        let memos = vec![audio::VoiceMemoEntry {
            id: "Memo #1".into(),
            filename: "memo.wav".into(),
            duration_ms: 100,
            transcript: None,
            title: None,
        }];
        assert!(has_extra_study_content(None, Some(&memos)));
        assert!(!has_extra_study_content(None, None));
    }

    #[test]
    fn notes_dot_includes_sketches_and_respects_locked_editor() {
        let notes = HashMap::from([(1, "saved text".to_owned())]);
        let mut sketches =
            HashMap::from([(2, notes::SketchPage::new(notes::BlankPageStyle::Blank))]);
        assert!(!page_has_notes_or_sketch(
            2,
            1,
            "editing page one",
            &notes,
            &sketches
        ));
        assert!(page_has_notes_or_sketch(
            1,
            1,
            "editing page one",
            &notes,
            &sketches
        ));
        assert!(!page_has_notes_or_sketch(1, 1, " ", &notes, &sketches));
        assert!(page_has_notes_or_sketch(1, 2, "", &notes, &sketches));
        sketches
            .get_mut(&2)
            .unwrap()
            .strokes
            .push(annotations::PageAnnotation {
                kind: annotations::AnnotationKind::Freehand,
                color: [0, 0, 0],
                points: vec![(1.0, 1.0), (4.0, 4.0)],
                text: String::new(),
                font_size: 18.0,
                bold: false,
                italic: false,
                stroke_width: 2.5,
                native_source: None,
            });
        assert!(page_has_notes_or_sketch(2, 1, "", &notes, &sketches));
    }

    #[test]
    fn opening_rejects_bad_or_empty_pdf_but_accepts_valid_pdf() {
        use lopdf::{Object, dictionary};
        let dir = std::env::temp_dir().join(format!("lapis-open-test-{}", docid::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.pdf");
        assert!(inspect_pdf(&path).is_err());
        std::fs::write(&path, b"not a PDF").unwrap();
        assert!(inspect_pdf(&path).is_err());
        for count in [0, 1] {
            let mut doc = lopdf::Document::with_version("1.5");
            let pages = doc.new_object_id();
            let mut kids = Vec::new();
            if count == 1 {
                let page = doc.add_object(dictionary! {
                    "Type" => "Page", "Parent" => Object::Reference(pages),
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                    "Resources" => dictionary! {},
                });
                kids.push(Object::Reference(page));
            }
            doc.objects.insert(
                pages,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages", "Kids" => kids, "Count" => Object::Integer(count),
                }),
            );
            let root = doc.add_object(
                dictionary! { "Type" => "Catalog", "Pages" => Object::Reference(pages) },
            );
            doc.trailer.set("Root", root);
            doc.save(&path).unwrap();
            let result = inspect_pdf(&path);
            if count == 0 {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().0, 1);
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
#[ignore = "manual benchmark using a dense document"]
fn workspace_performance_fixture() {
    use crate::test_support::isolated_dir;
    use std::hint::black_box;
    use std::time::Instant;

    let root = isolated_dir!(workspace_performance_fixture);
    let pdf = root.join("workspace.pdf");
    notes::tests::write_save_fixture_pdf(&pdf, 420, 1500, true);
    let mut data = notes::DocData::default();
    let mut stroke = notes::tests::sample_textbox(
        "",
        (0..40)
            .map(|i| (0.05 + 0.02 * i as f32, 0.4 + 0.01 * (i % 5) as f32))
            .collect(),
    );
    stroke.kind = AnnotationKind::Freehand;
    for page in 1..=420 {
        data.notes.insert(page, "Example study notes.\n".repeat(48));
        data.annotations.insert(page, vec![stroke.clone(); 20]);
        if page % 7 == 1 {
            let mut sketch = notes::SketchPage::new(notes::BlankPageStyle::Grid);
            sketch.strokes.push(stroke.clone());
            data.sketches.insert(page, sketch);
        }
    }
    notes::save_doc_data(
        &pdf,
        &data.notes,
        &data.annotations,
        data.blank_pages,
        &data.sketches,
    )
    .unwrap();

    let ctx = egui::Context::default();
    let mut app = Lapis::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Some(pdf.clone()),
    );
    app.sync_local_checkpoint().unwrap();
    fn timed(label: &str, work: impl FnOnce()) {
        let start = Instant::now();
        work();
        println!(
            "workspace,{label},{:.6}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    timed("120 idle checkpoint checks", || {
        for _ in 0..120 {
            black_box(&mut app).checkpoint_after_frame(black_box(&ctx));
        }
    });
    timed("30 reading position updates and flush", || {
        for page in 2..=31 {
            app.current_page = page;
            app.save_session_state();
        }
        app.flush_session_state();
    });
    assert_eq!(AppState::load().find_entry(&pdf).unwrap().last_page, 31);
    let search_started = Instant::now();
    timed("first PDF search index request", || {
        app.ensure_pdf_search_index(&ctx)
    });
    app.pdf_search_index.wait_for_result();
    println!(
        "workspace,first PDF search index ready,{:.6}",
        search_started.elapsed().as_secs_f64() * 1000.0
    );
}

#[test]
fn checkpoint_tracks_completed_edits_and_retries_a_failed_write() {
    use crate::test_support::isolated_dir;
    use std::time::{Duration, Instant};

    let root = isolated_dir!(checkpoint_tracks_completed_edits_and_retries_a_failed_write);
    let pdf = root.join("checkpoint.pdf");
    notes::create_blank_study_notebook(&pdf).unwrap();
    let ctx = egui::Context::default();
    let mut app = Lapis::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Some(pdf.clone()),
    );
    app.current_note = "Before the failed write".into();
    app.checkpoint_after_frame(&ctx);
    assert!(!app.has_uncheckpointed_edits());

    let backup = notes::get_backup_path(&pdf);
    let blocked = backup.with_extension("json.tmp");
    std::fs::create_dir(&blocked).unwrap();
    app.current_note = "Edited before leaving this note".into();
    app.checkpoint_after_frame(&ctx);
    assert!(app.checkpoint_failed_at.is_some());
    assert!(app.has_uncheckpointed_edits());
    assert_eq!(
        notes::load_doc_data(&pdf).notes[&1],
        "Before the failed write"
    );

    std::fs::remove_dir(&blocked).unwrap();
    app.checkpoint_failed_at = Some(Instant::now() - Duration::from_secs(3));
    app.checkpoint_after_frame(&ctx);
    assert!(!app.has_uncheckpointed_edits());
    assert_eq!(
        notes::load_doc_data(&pdf).notes[&1],
        "Edited before leaving this note"
    );

    app.push_undo_snapshot(1);
    let stroke = notes::tests::sample_textbox("Annotation", vec![(0.1, 0.1), (0.3, 0.3)]);
    app.annotations.insert(1, vec![stroke]);
    app.mark_study_changed();
    app.checkpoint_after_frame(&ctx);
    assert_eq!(notes::load_doc_data(&pdf).annotations[&1].len(), 1);
    app.undo_annotation(1);
    assert!(!app.has_uncheckpointed_edits());
    assert!(notes::load_doc_data(&pdf).annotations.is_empty());
}

#[test]
fn reading_position_updates_coalesce_and_flush_before_document_switch() {
    use crate::test_support::isolated_dir;

    let root = isolated_dir!(reading_position_updates_coalesce_and_flush_before_document_switch);
    let first = root.join("first.pdf");
    let second = root.join("second.pdf");
    notes::tests::write_save_fixture_pdf(&first, 8, 0, false);
    notes::create_blank_study_notebook(&second).unwrap();
    let ctx = egui::Context::default();
    let mut app = Lapis::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Some(first.clone()),
    );
    assert!(app.session_save_due.is_none());

    for page in 2..=7 {
        app.go_to_page(page, &ctx);
    }
    assert_eq!(AppState::load().find_entry(&first).unwrap().last_page, 1);
    assert_eq!(app.app_state.find_entry(&first).unwrap().last_page, 7);
    assert!(app.session_save_due.is_some());
    assert!(app.open_pdf(second.clone(), &ctx));
    let state = AppState::load();
    assert_eq!(state.find_entry(&first).unwrap().last_page, 7);
    assert_eq!(
        state.recent_files.front().unwrap().path,
        dunce::canonicalize(&second).unwrap()
    );
    assert!(app.session_save_due.is_none());
    app.save_session_state();
    assert!(app.session_save_due.is_none());
}

#[test]
fn leaving_waits_for_edits_made_after_the_pdf_snapshot() {
    use crate::test_support::isolated_dir;
    use std::time::{Duration, Instant};

    let root = isolated_dir!(leaving_waits_for_edits_made_after_the_pdf_snapshot);
    let pdf = root.join("leave.pdf");
    notes::create_blank_study_notebook(&pdf).unwrap();
    let ctx = egui::Context::default();
    let mut app = Lapis::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Some(pdf.clone()),
    );
    let service = |app: &mut Lapis| {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.service_pdf_save(ui);
        });
        output.textures_delta.clear();
    };

    app.current_note = "First snapshot".into();
    app.request_pdf_save(AfterPdfSave::Home, &ctx);
    service(&mut app);
    assert!(app.pdf_save_job.is_some());
    app.current_note = "Edited while the PDF was saving".into();
    app.checkpoint_after_frame(&ctx);

    let deadline = Instant::now() + Duration::from_secs(10);
    while app.has_pdf() && app.pdf_save_failure.is_none() && Instant::now() < deadline {
        service(&mut app);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !app.has_pdf(),
        "The newest snapshot must be committed before leaving"
    );
    std::fs::remove_file(notes::get_backup_path(&pdf)).unwrap();
    assert_eq!(
        notes::load_doc_data(&pdf).notes[&1],
        "Edited while the PDF was saving"
    );
}
