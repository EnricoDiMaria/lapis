use super::*;
use crate::{notes, test_support::isolated_dir};
use std::sync::mpsc::{Receiver, Sender, channel};

fn connect_workers(
    viewer: &mut Viewer,
) -> (
    Receiver<RenderRequest>,
    Receiver<RenderRequest>,
    Sender<WorkerMsg>,
) {
    let (page_requests, pages) = channel();
    let (thumbnail_requests, thumbnails) = channel();
    let (sender, results) = channel();
    viewer.workers = Some(RenderWorkers {
        page_requests,
        thumbnail_requests,
        results,
    });
    (pages, thumbnails, sender)
}

fn image(page: u32, is_thumb: bool) -> WorkerMsg {
    WorkerMsg::Page(RenderResult {
        page_idx: (page - 1) as i32,
        scale_key: 100,
        page_pt: (2.0, 2.0),
        image: egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
        is_thumb,
        dark_mode: false,
        links: Vec::new(),
    })
}

fn test_view() -> RenderView {
    RenderView {
        current_page: 5,
        total_pages: 8,
        continuous_visible_pages: None,
        split_page: None,
    }
}

#[test]
fn retiring_workers_disconnects_both_queues_and_discards_pending_results() {
    let ctx = egui::Context::default();
    let mut viewer = Viewer::default();
    let view = test_view();
    let (pages, thumbnails, old_results) = connect_workers(&mut viewer);
    old_results.send(image(view.current_page, false)).unwrap();
    viewer.stop_workers();
    assert!(matches!(
        pages.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
    assert!(matches!(
        thumbnails.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
    assert!(old_results.send(image(view.current_page, false)).is_err());

    let (_, _, new_results) = connect_workers(&mut viewer);
    assert!(viewer.upload_ready(&ctx, view, false).is_none());
    assert!(viewer.page(view.current_page).is_none());
    new_results.send(image(view.current_page, false)).unwrap();
    assert!(viewer.upload_ready(&ctx, view, false).is_some());
}

#[test]
fn requests_prioritize_visible_pages_and_deduplicate_unchanged_state() {
    let ctx = egui::Context::default();
    let mut viewer = Viewer::default();
    let (pages, thumbnails, results) = connect_workers(&mut viewer);
    let view = RenderView {
        current_page: 5,
        continuous_visible_pages: Some((4, 6)),
        split_page: Some(2),
        ..test_view()
    };
    results.send(image(5, false)).unwrap();
    viewer.upload_ready(&ctx, view, false);

    viewer.request_pages(view, 1.0, false);
    let request = pages.try_recv().unwrap();
    assert_eq!(request.pages, [3, 5, 6, 2, 7, 1]);
    viewer.request_pages(view, 1.0, false);
    assert!(matches!(
        pages.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    viewer.request_pages(view, 1.0, true);
    let request = pages.try_recv().unwrap();
    assert_eq!(request.pages, [4, 3, 5, 6, 2, 7, 1]);
    assert!(request.dark_mode);

    viewer.request_thumbnails(8, 3, 5, false);
    let request = thumbnails.try_recv().unwrap();
    assert_eq!(request.pages, [4, 3, 5, 2, 6, 1, 7, 0]);
    assert_eq!(request.scale, THUMB_SCALE);
    viewer.request_thumbnails(8, 3, 5, false);
    assert!(matches!(
        thumbnails.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn upload_limits_images_per_frame_and_falls_back_to_the_same_page_thumbnail() {
    let ctx = egui::Context::default();
    let mut viewer = Viewer::default();
    let (_, _, results) = connect_workers(&mut viewer);
    let view = test_view();
    results.send(image(4, false)).unwrap();
    results
        .send(WorkerMsg::Text {
            page_idx: 4,
            text: Arc::default(),
        })
        .unwrap();
    results.send(image(5, true)).unwrap();
    results.send(image(5, false)).unwrap();

    assert!(viewer.upload_ready(&ctx, view, false).is_none());
    assert!(viewer.text(5).is_some());
    assert!(viewer.page(4).is_some());
    assert!(viewer.page(5).is_none());
    assert!(viewer.page_or_thumbnail(3).is_none());
    let thumbnail = viewer.thumbnail(5).unwrap().0.id();
    assert_eq!(viewer.page_or_thumbnail(5).unwrap().0.id(), thumbnail);

    let render = viewer.upload_ready(&ctx, view, false).unwrap();
    assert_eq!(render.page_pt, (2.0, 2.0));
    assert_eq!(
        viewer.page_or_thumbnail(5).unwrap().0.id(),
        render.texture.0.id()
    );
    assert_ne!(render.texture.0.id(), thumbnail);
}

#[test]
fn render_cache_keeps_requested_pages_and_evicts_the_oldest_other_texture() {
    let ctx = egui::Context::default();
    let mut viewer = Viewer::default();
    let (_, _, results) = connect_workers(&mut viewer);
    let view = test_view();
    for page in [1, 2, 4, 5] {
        results.send(image(page, false)).unwrap();
    }
    viewer.upload_ready(&ctx, view, false);
    viewer.upload_ready(&ctx, view, false);
    viewer.touch_page(1);
    viewer.evict_pages(view, 3 * 16);
    assert!(viewer.page(1).is_some());
    assert!(viewer.page(2).is_none());
    assert!(viewer.page(4).is_some());
    assert!(viewer.page(5).is_some());
    assert_eq!(viewer.pages.bytes, 3 * 16);
    results.send(image(5, false)).unwrap();
    viewer.upload_ready(&ctx, view, false);
    assert_eq!(viewer.pages.bytes, 3 * 16);
    viewer.invalidate_images();
    assert_eq!(viewer.pages.bytes, 0);
}

#[test]
fn thumbnails_respect_the_byte_budget_and_keep_the_current_page_when_possible() {
    let ctx = egui::Context::default();
    let mut viewer = Viewer::default();
    let (_, _, results) = connect_workers(&mut viewer);
    let view = test_view();
    for page in [5, 1, 2, 6] {
        results.send(image(page, true)).unwrap();
    }
    viewer.upload_ready(&ctx, view, false);
    viewer.upload_ready(&ctx, view, false);
    viewer.evict_thumbnails(view, 2 * 16);
    assert_eq!(viewer.thumbnails.bytes, 2 * 16);
    assert!(viewer.thumbnail(5).is_some());
    assert!(viewer.thumbnail(6).is_some());
    assert!(viewer.thumbnail(1).is_none());
    assert!(viewer.thumbnail(2).is_none());

    viewer.invalidate_images();
    results.send(image(5, true)).unwrap();
    viewer.upload_ready(&ctx, view, true);
    assert_eq!(viewer.thumbnails.bytes, 0);
    assert!(viewer.thumbnail(5).is_none());
}

#[test]
fn render_workers_send_the_page_image_before_text_and_render_thumbnails_separately() {
    use std::time::Duration;

    let root = isolated_dir!(
        render_workers_send_the_page_image_before_text_and_render_thumbnails_separately
    );
    let pdf = root.join("lecture.pdf");
    notes::create_blank_study_notebook(&pdf).unwrap();
    let ctx = egui::Context::default();
    let RenderWorkers {
        page_requests: pages,
        thumbnail_requests: thumbs,
        results,
    } = RenderWorkers::start(pdf, ctx);
    let timeout = Duration::from_secs(5);
    pages
        .send(RenderRequest {
            scale: 0.5,
            pages: vec![0],
            dark_mode: true,
        })
        .unwrap();
    let WorkerMsg::Page(image) = results.recv_timeout(timeout).unwrap() else {
        panic!("The first result should be an image");
    };
    assert_eq!(image.page_idx, 0);
    assert_eq!(image.image.size, [480, 270]);
    assert!(image.dark_mode);
    assert!(!image.is_thumb);
    assert!(matches!(
        results.recv_timeout(timeout).unwrap(),
        WorkerMsg::Text { page_idx: 0, .. }
    ));

    thumbs
        .send(RenderRequest {
            scale: 0.25,
            pages: vec![0],
            dark_mode: false,
        })
        .unwrap();
    let WorkerMsg::Page(thumbnail) = results.recv_timeout(timeout).unwrap() else {
        panic!("The thumbnail worker should return an image");
    };
    assert_eq!(thumbnail.page_idx, 0);
    assert_eq!(thumbnail.image.size, [240, 135]);
    assert!(!thumbnail.dark_mode);
    assert!(thumbnail.is_thumb);
    thumbs
        .send(RenderRequest {
            scale: 1.0,
            pages: vec![0],
            dark_mode: false,
        })
        .unwrap();
    let WorkerMsg::Page(capped) = results.recv_timeout(timeout).unwrap() else {
        panic!("The thumbnail worker should return an image");
    };
    assert_eq!(capped.image.size, [512, 288]);
    drop((pages, thumbs));
    assert!(matches!(
        results.recv_timeout(timeout),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
}
